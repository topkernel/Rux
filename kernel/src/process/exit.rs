//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Process/thread exit and wait implementation
//!
//! - do_exit: termination with thread-group semantics
//!   - non-leader thread: ring detach + deferred release (no ZOMBIE —
//!     threads are never wait4()ed; Linux delay_put analogue)
//!   - leader (or single-threaded): force SIGKILL remaining members,
//!     wait for the group to drain, then the classic ZOMBIE flow
//! - do_exit_group / de_thread: group teardown entry points (syscall 94,
//!   execve, fatal default signals)
//! - release_task: Reap zombie child resources (incl. exited group members)
//! - do_wait / do_wait_nonblock / do_waitid: wait for child state change

use crate::errno;
use crate::process::task::{Pid, Task, TaskState};
use core::arch::asm;

// ============================================================================
// Robust list / clear_child_tid exit processing (must run while the mm lives)
// ============================================================================

/// FUTEX_OWNER_DIED bit (Linux include/uapi/linux/futex.h).
const FUTEX_OWNER_DIED: u32 = 0x4000_0000;
/// TID mask of a futex word (low 30 bits hold the owner TID).
const FUTEX_TID_MASK: u32 = 0x3fff_ffff;
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
unsafe fn exit_robust_list(task: *mut Task) {
    use crate::arch::uaccess::get_user;

    let head = (*task).robust_list_head() as usize;
    if head == 0 || (*task).robust_list_len() != ROBUST_LIST_HEAD_SIZE {
        return; // not registered / legacy len — nothing to do
    }
    #[cfg(feature = "dfx-futex-trace")]
    {
        use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
        taskdump_raw_line(b"FTX ROBUST-EXIT pid=");
        taskdump_dec((*task).pid() as u64);
        taskdump_raw_line(b" head=");
        taskdump_dec(head as u64);
        taskdump_raw_line(b"\n");
    }
    let my_tid = (*task).pid();
    let mm_id = crate::sync::futex::task_futex_mm_id(task);

    // futex_offset (signed long) at head+8: uaddr = entry + futex_offset.
    let futex_offset: i64 = match get_user((head + 8) as *const i64) {
        Some(v) => v,
        None => return,
    };

    // Process one entry: mark OWNER_DIED if we still own it, wake waiters.
    // SAFETY: all user accesses go through the exception-table helpers.
    unsafe fn handle_robust_entry(entry: usize, futex_offset: i64, tid: u32, mm_id: usize) {
        use crate::arch::uaccess::{get_user, put_user};
        let uaddr = (entry as i64).wrapping_add(futex_offset) as u64;
        if uaddr & 0x3 != 0 {
            return; // misaligned futex word — skip
        }
        let v: u32 = match get_user(uaddr as *const u32) {
            Some(v) => v,
            None => return, // bad pointer — skip entry
        };
        if v & FUTEX_TID_MASK == tid {
            let _ = put_user(uaddr as *mut u32, v | FUTEX_OWNER_DIED);
            crate::sync::futex::futex_wake_in_mm(
                uaddr as usize,
                mm_id,
                0, // private futex semantics
                1,
                u32::MAX,
            );
        }
    }

    // Walk a robust_list chain starting at `start` (an entry address);
    // the chain ends when next == head (circular) or is unreadable.
    // SAFETY: bounded walk, exception-table user reads.
    unsafe fn walk_robust_chain(start: usize, head: usize, futex_offset: i64, tid: u32, mm_id: usize) {
        use crate::arch::uaccess::get_user;
        let mut entry = start;
        let mut count = 0u32;
        while entry != 0 && entry != head && count < ROBUST_LIST_LIMIT {
            handle_robust_entry(entry, futex_offset, tid, mm_id);
            entry = match get_user(entry as *const u64) {
                Some(next) => next as usize,
                None => break,
            };
            count += 1;
        }
    }

    // Main list: first entry hangs off head+0.
    // SAFETY: bounded, exception-table reads.
    unsafe {
        if let Some(first) = get_user(head as *const u64) {
            walk_robust_chain(first as usize, head, futex_offset, my_tid, mm_id);
        }
        // Pending list (held-when-killed locks): pointer at head+16; the
        // pointed-to node is itself an entry.
        if let Some(pending) = get_user((head + 16) as *const u64) {
            if pending as usize != 0 {
                walk_robust_chain(pending as usize, head, futex_offset, my_tid, mm_id);
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
unsafe fn exit_clear_child_tid(task: *mut Task) {
    let tid_ptr = (*task).clear_child_tid();
    if tid_ptr.is_null() {
        return;
    }
    let mm_id = crate::sync::futex::task_futex_mm_id(task);
    #[cfg(feature = "dfx-futex-trace")]
    {
        use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
        taskdump_raw_line(b"FTX CLEARTID-EXIT pid=");
        taskdump_dec((*task).pid() as u64);
        taskdump_raw_line(b" u=");
        taskdump_dec(tid_ptr as usize as u64);
        taskdump_raw_line(b"\n");
    }

    // Write 0 to the tid pointer in user memory.
    let zero: i32 = 0;
    crate::arch::uaccess::copy_to_user(
        tid_ptr as *mut u8,
        &zero as *const i32 as *const u8,
        core::mem::size_of::<i32>(),
    );
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
    (*task).set_clear_child_tid(core::ptr::null_mut());
}

// ============================================================================
// Thread group teardown
// ============================================================================

/// Force SIGKILL every OTHER live member of the caller's thread group.
/// Idempotent; the caller then waits via wait_for_thread_group_death.
pub fn kill_other_threads(current: *mut Task) {
    // Collect member pids under the ring lock; deliver outside it.
    let mut victims: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
    // SAFETY: current is a valid, running task; its ring is well-formed.
    unsafe {
        (*current).for_each_group_member(|m| {
            if m != current {
                let st = (*m).state();
                if !st.is_dead() {
                    victims.push((*m).pid());
                }
            }
        });
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
pub fn wait_for_thread_group_death(current: *mut Task) {
    crate::arch::cpu::restore_irq(true);
    // SAFETY: current stays valid (it is the running task).
    while unsafe { (*current).nr_threads() } > 1 {
        crate::sched::schedule();
    }
}

/// de_thread — exec-time group teardown (Linux de_thread):
/// kill every other thread and wait for the group to drain before the
/// new image is installed. Called from the execve path AFTER all
/// fail-able preparation (a failed exec leaves the process single-
/// threaded, matching Linux's point-of-no-return behavior).
pub fn de_thread(current: *mut Task) {
    // SAFETY: caller (execve path) passes the running task.
    unsafe {
        if (*current).nr_threads() > 1 {
            kill_other_threads(current);
            wait_for_thread_group_death(current);
        }
    }
}

/// Free one exited group member whose context switch-out has completed.
///
/// Mirrors release_task's tail for a member: the member already removed
/// itself from the pid hash, freed its PID, and dropped its mm/fdtable
/// Arcs at exit time — here we only wait out on_cpu, drop remaining Arcs
/// (signal/fs), free the kernel stack, and put the Task slot.
///
/// # Safety
/// `member` is a DEAD task parked on a leader's dead_threads list (callers
/// guarantee the DEAD state — see sweep_dead_threads / release_task).
unsafe fn free_dead_member(member: *mut Task) {
    // The member's final context switch-out may still be in flight —
    // on_cpu is cleared exactly when __switch_to has saved its context
    // (see release_task R9 for the full rationale).
    crate::arch::cpu::restore_irq(true);
    // DFX (4-thread hang hunt): report a stuck wait loudly instead of
    // spinning silently — a member whose on_cpu never clears wedges every
    // later exiter (and their CPUs) here.
    {
        let mut spins: u32 = 0;
        while (*member).on_cpu() {
            if spins == 0 {
                let cur_pid = crate::sched::current().map(|c| c.pid()).unwrap_or(0);
                crate::dfx::taskdump::taskdump_raw_line(b"FDM-WAIT sweeper=");
                crate::dfx::taskdump::taskdump_dec(cur_pid as u64);
                crate::dfx::taskdump::taskdump_raw_line(b" member=");
                crate::dfx::taskdump::taskdump_dec((*member).pid() as u64);
                crate::dfx::taskdump::taskdump_raw_line(b" state=0x");
                let st = (*member).state().bits() as u64;
                let mut sh: i32 = 32;
                while sh > 0 {
                    sh -= 4;
                    let nb = ((st >> sh) & 0xF) as u8;
                    crate::dfx::taskdump::taskdump_raw_line(&[(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 })]);
                }
                crate::dfx::taskdump::taskdump_raw_line(b"\n");
            }
            spins = spins.wrapping_add(1);
            core::hint::spin_loop();
        }
    }
    crate::sync::rcu::synchronize_rcu();

    (*member).free_kernel_stack();

    // Heap PtRegs from the legacy fork path (unused by the live path).
    let pt_regs_ptr = (*member).fork_pt_regs();
    if !pt_regs_ptr.is_null() {
        use alloc::alloc::{dealloc, Layout};
        let layout = Layout::from_size_align(
            core::mem::size_of::<crate::arch::pt_regs::PtRegs>(), 16
        ).unwrap();
        dealloc(pt_regs_ptr as *mut u8, layout);
    }

    // Drop remaining Arc references (mm/fdtable were dropped at exit).
    (*member).set_address_space(None);
    (*member).set_fdtable(None);
    (*member).signal = None;
    (*member).set_fs(None);

    crate::process::task::Task::task_put(member);
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
    let pending: alloc::vec::Vec<*mut Task> = {
        // SAFETY: leader is a valid task; dead_threads is lock-guarded.
        let mut dl = unsafe { (*leader).dead_threads.lock() };
        let old = core::mem::replace(&mut *dl, alloc::vec::Vec::new());
        let mut still_running: alloc::vec::Vec<*mut Task> =
            alloc::vec::Vec::with_capacity(old.len());
        let mut reclaimable: alloc::vec::Vec<*mut Task> = alloc::vec::Vec::new();
        for m in old {
            // SAFETY: m is a parked member pointer (pushed by its own do_exit
            // while holding the task alive via the dead-list reference).
            let dead_and_off_cpu = unsafe {
                (*m).state().is_dead() && !(*m).on_cpu()
            };
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
        // final switch-out completed).
        unsafe { free_dead_member(m) };
    }
}

// ============================================================================
// Reaping
// ============================================================================

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
// SAFETY: Caller guarantees task is a zombie and no CPU is running it. The RCU
// grace period in synchronize_rcu() ensures no readers hold stale references.
pub(crate) unsafe fn release_task(task: *mut Task) {
    // A reaped leader first drains its dead-members list — every exited
    // group member is fully released before the leader's own storage
    // goes away (their group_leader pointers name `task`).
    //
    // A member still mid-exit (parked but preempted before its terminal
    // DEAD store — RUNNING/queued, fully alive) must NOT be freed. The
    // leader's ZOMBIE flow (wait_for_thread_group_death) normally
    // guarantees every member is DEAD by now, but apply the same exact
    // partition as sweep_dead_threads anyway: only genuinely DEAD members
    // are freed; live stragglers are parked back on the list and retried
    // (they are runnable and reach DEAD through their own do_exit —
    // free_dead_member then waits out the final on_cpu window).
    loop {
        let (reclaim, stragglers) = {
            let mut guard = (*task).dead_threads.lock();
            let members = core::mem::replace(&mut *guard, alloc::vec::Vec::new());
            drop(guard);
            let mut reclaim = alloc::vec::Vec::new();
            let mut stragglers = alloc::vec::Vec::new();
            for m in members {
                // SAFETY: m is a parked member pointer owned by this list.
                if (*m).state().is_dead() {
                    reclaim.push(m);
                } else {
                    stragglers.push(m);
                }
            }
            (reclaim, stragglers)
        };
        for m in reclaim {
            free_dead_member(m);
        }
        if stragglers.is_empty() {
            break;
        }
        // Park stragglers back and let them run — mid-exit members are
        // runnable (RUNNING) and will reach DEAD on their own CPU.
        let mut guard = (*task).dead_threads.lock();
        for m in stragglers {
            guard.push(m);
        }
        drop(guard);
        // Yield so the stragglers can progress (IRQs are on throughout).
        crate::arch::cpu::restore_irq(true);
        core::hint::spin_loop();
    }

    // Remove from PID hash table before freeing resources
    crate::process::pid_hash::pid_hash_remove((*task).pid());

    // Wait for any RCU readers that may still be traversing this task's
    // pid_hash_links node to finish before we free the task memory.
    crate::sync::rcu::synchronize_rcu();

    // Detach from parent's children list (must happen before freeing task memory)
    // E8-REPAR: screen the parent pointer first — do_wait claimed the
    // child under the tree lock (parent forced None), so a Some here is
    // either the normal pre-claim path or a stale pointer planted by
    // older corruption; never dereference an implausible one.
    let parent_ptr = (*task).parent_ptr();
    if let Some(parent) = parent_ptr {
        if Task::is_plausible_task_ptr(parent) {
            (*parent).remove_child(task);
        }
    }

    // R9 (NEW2 engine #2): do_exit sets ZOMBIE BEFORE its final schedule(),
    // so a reaping parent could scan the early ZOMBIE and free this task's
    // kernel stack and Task while it is STILL EXECUTING its exit tail on
    // another CPU. The stack/Task would be reused (and zeroed) by the next
    // fork, and the dying task's remaining stores — including __switch_to's
    // context save — would land in live objects (the corrupted wait4
    // statuses, EBADF, zeroed children-list nodes). on_cpu is cleared by
    // __switch_to exactly when this task's context is saved: wait for that
    // before freeing. Bounded: the task is on its way out; no locks held.
    // R9-16 (actually landed, round 10): spin with interrupts enabled —
    // this CPU must keep reporting RCU quiescent states and taking ticks
    // while waiting (the round-9 commit claimed this but never landed).
    crate::arch::cpu::restore_irq(true);
    while (*task).on_cpu() {
        core::hint::spin_loop();
    }

    // Free kernel stack
    (*task).free_kernel_stack();

    // Free heap-allocated PtRegs from fork.
    // copy_thread() allocates PtRegs on the heap for the child's first
    // context switch; clear_fork_child() keeps the pointer so we can free it here.
    let pt_regs_ptr = (*task).fork_pt_regs();
    if !pt_regs_ptr.is_null() {
        use alloc::alloc::{dealloc, Layout};
        let layout = Layout::from_size_align(
            core::mem::size_of::<crate::arch::pt_regs::PtRegs>(), 16
        ).unwrap();
        dealloc(pt_regs_ptr as *mut u8, layout);
    }

    // Clear Arc references (this will decrement reference counts)
    (*task).set_address_space(None);
    (*task).set_fdtable(None);
    (*task).signal = None;
    (*task).set_fs(None);

    // Free PID
    crate::process::pid::free_pid((*task).pid());

    // Free Task struct back to kernel heap — via task_put (R12-5): any
    // outstanding lookup pins delay the actual free to their own put.
    crate::process::task::Task::task_put(task);
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
pub fn do_exit(exit_code: i32) -> ! {
    let current = match crate::sched::current() {
        Some(c) => c as *mut Task,
        None => {
            loop {
                // SAFETY: `wfi` is a plain hint instruction with no side effects.
                unsafe { asm!("wfi", options(nomem, nostack)); }
            }
        }
    };

    // SAFETY: current is the raw pointer to the calling task, guaranteed valid since
    // we are the currently executing task and will never return from this function.
    unsafe {
        let current_pid = (*current).pid();
        let leader = (*current).group_leader_ptr();
        let is_leader = leader == current;
        let parent_pid = if is_leader {
            (*current).ppid()
        } else {
            // Threads share the leader's real parent.
            (*leader).ppid()
        };

        // ===== Journal handle unwind =====
        // A task killed mid-syscall (SIGKILL/SIGSEGV between
        // set_current_handle and clear_current_handle — LTP abort01 sends
        // SIGABRT into its children) dies owning a jbd2 handle: its
        // transaction's t_updates stays elevated forever, and EVERY later
        // jbd2_journal_stop spins out its 1,000,000-lap "wait for other
        // handles" loop (seconds under TCG — the capset01/03 soft-lockup
        // family; multiplied per write it became a whole-chunk wedge).
        // Unwind the reference now: the handle lives on this task's dying
        // stack and can never reach ext4_journal_stop itself. The
        // registered dirty buffers stay in the transaction for the next
        // stopper to commit; nothing else in the handle needs release.
        if let Some(handle_ptr) = crate::fs::ext4::namei::get_current_handle() {
            let txn = (*handle_ptr).h_transaction.clone();
            if let Some(txn) = txn {
                txn.t_updates.fetch_sub(1, core::sync::atomic::Ordering::SeqCst);
            }
            crate::fs::ext4::namei::clear_current_handle();
        }

        // ===== Ptrace tracer death: auto-detach tracees =====
        // Linux forgets the tracing relationship when the tracer exits
        // (ptrace_exit → every tracee is detached and resumed). Without
        // this, a tracer that dies between PTRACE_ATTACH and DETACH (the
        // LTP runner 30s-kills a wedged test) leaves its tracee STOPPED
        // forever — when the tracee is init (ptrace11 attaches PID 1) the
        // whole guest wedges.
        {
            crate::process::pid_hash::pid_hash_for_each_task(|t| unsafe {
                if (*t).tracer_pid() != current_pid || (*t).pid() == current_pid {
                    return;
                }
                // SAFETY: task is alive for the duration of the hash walk
                // (same discipline as send_signal_to_pgid's in-walk use).
                (*t).set_tracer_pid(0);
                (*t).set_ptrace_options(0);
                (*t).clear_single_step();
                if (*t).state().contains(TaskState::STOPPED) {
                    crate::signal::signal_wake_up(t);
                }
            });
        }

        // ===== Core dump (P1) =====
        // A negative exit code encodes death by signal; the core-dumping
        // signals get a core file written BEFORE any resource is torn
        // down. Runs in the dying task's context (its cwd/fdtable/mm are
        // still intact). This is the single funnel for both the signal
        // default-action path (handle_default_signal -> do_exit_group)
        // and the direct trap paths (SIGSEGV/SIGILL/... -> do_exit).
        if exit_code < 0 && crate::process::coredump::signal_makes_core(-exit_code) {
            crate::process::coredump::do_coredump(current, -exit_code);
        }

        crate::pr_debug!("exit: pid={}, tgid={}, exit_code={}, ppid={} ({})",
            current_pid, (*current).tgid(), exit_code, parent_pid,
            if is_leader { "leader" } else { "thread" });

        // DFX (dbus-death hunt): every death-by-signal gets a KERN_INFO
        // line on the console — the default loglevel hides pr_debug, and
        // the only external symptom of the dbus-daemon kill was
        // ECONNREFUSED with the socket file left behind. `kill -0`-style
        // exits (normal, code>=0) are not printed: the fork/exec storm
        // would bury the signal deaths in noise.
        if exit_code < 0 {
            let comm_raw = (*current).comm();
            let comm_len = comm_raw.iter().position(|&b| b == 0).unwrap_or(comm_raw.len());
            // Sender forensics: SI_USER deaths (kill/tgkill) carry the
            // sender's pid in the queued siginfo — print it so "who killed
            // X" is answerable from the console (kernel-originated signals
            // have no sender and print si_code instead).
            let sender = (*current)
                .pending
                .peek_info(-exit_code)
                .map(|si| (si.si_pid, si.si_code))
                .unwrap_or((0, 0));
            crate::pr_info!(
                "SIGDEATH: pid={} tgid={} comm={:?} sig={} ppid={}{} si_pid={} si_code={:#x}",
                current_pid,
                (*current).tgid(),
                core::str::from_utf8(&comm_raw[..comm_len]),
                -exit_code,
                parent_pid,
                if is_leader { "" } else { " (thread)" },
                sender.0,
                sender.1,
            );
        }

        #[cfg(feature = "dfx-futex-trace")]
        {
            use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
            taskdump_raw_line(b"FTX EXIT pid=");
            taskdump_dec(current_pid as u64);
            taskdump_raw_line(b" code=");
            taskdump_dec(exit_code as i64 as u64);
            taskdump_raw_line(b" threads=");
            taskdump_dec((*current).nr_threads() as u64);
            taskdump_raw_line(b"\n");
        }

        // Set exit code
        (*current).set_exit_code(exit_code);

        // ===== CPU-time accounting fold =====
        // Linux semantics: a dying thread's user/system ticks fold into the
        // thread-group leader (so /proc/[pid]/stat, times() and
        // getrusage(RUSAGE_SELF) report group totals), and the leader's
        // accumulated totals fold into the PARENT's cutime/cstime (what
        // times()/getrusage(RUSAGE_CHILDREN) report after wait()). Lost
        // silently when the parent already exited (orphan), matching the
        // fact that nobody can wait for them anymore.
        {
            use core::sync::atomic::Ordering::Relaxed;
            let u = (*current).utime_ticks.load(Relaxed);
            let s = (*current).stime_ticks.load(Relaxed);
            // ru_maxrss propagation (Linux signal->cmaxrss): the child's
            // resident set at exit (captured below while the mm is still
            // alive) folds into the parent's RUSAGE_CHILDREN hiwater.
            let rss_kb = {
                let mm_rss = (*current)
                    .address_space()
                    .map(|mm| mm.rss())
                    .unwrap_or(0);
                mm_rss * (crate::mm::page::PAGE_SIZE / 1024) as u64
            };
            (*current).exit_maxrss_kb.store(rss_kb, Relaxed);
            if is_leader {
                if let Some(parent) = crate::process::find_task_by_pid(parent_pid) {
                    (*parent).cutime_ticks.fetch_add(u, Relaxed);
                    (*parent).cstime_ticks.fetch_add(s, Relaxed);
                    let _ = (*parent).cmaxrss_kb.fetch_max(rss_kb, Relaxed);
                }
            } else {
                (*leader).utime_ticks.fetch_add(u, Relaxed);
                (*leader).stime_ticks.fetch_add(s, Relaxed);
            }
        }

        // Drop any in-progress signal frame: a handler that longjmp'd out
        // left sigframe set, which blocks all further delivery including
        // SIGKILL (review H15) — clear it so the dying task stays killable.
        (*current).sigframe = None;
        (*current).sigframe_addr = 0;

        // ===== Robust list + clear_child_tid: BEFORE the mm ref drops =====
        // Both write into user memory and futex-wake with THIS task's mm
        // as the key identity; once set_address_space(None) runs the key
        // is unobtainable and the user words unreachable.
        exit_robust_list(current);
        exit_clear_child_tid(current);

        // ===== cgroup v2 (U1b): leave the cgroup + settle memory =====
        // Drops cgroup membership (pids.current / cgroup.procs) and
        // uncharges the task's mmap(2) memory ledger — see
        // sched::cgroup::cgroup_exit for why the ledger, not total_vm,
        // is the uncharge unit.
        crate::sched::cgroup::cgroup_exit(current);

        // ===== exit_mm: drop THIS task's mm reference =====
        // Shared-mm teardown rules:
        // - shm detach: only the LAST reference holder (threads must not
        //   each decrement nattch).
        // - satp switch to the kernel root page table: EVERY dying task on
        //   a user page table, last or not. After clear_child_tid/robust
        //   list nothing touches user memory, and a dying group member
        //   holding the last reference FREES the page tables while this
        //   task may still be spinning in wait_for_thread_group_death on
        //   another CPU — running on a freed root is the ARCH-H4 fault.
        if let Some(as_arc) = (*current).address_space_arc() {
            let is_last = alloc::sync::Arc::strong_count(&as_arc) == 1;
            if is_last {
                // Iterate VMAs to detach shared memory segments (decrement nattch)
                let vma_mgr = as_arc.vma_read();
                for vma in vma_mgr.iter() {
                    if vma.vma_type() == crate::mm::vma::VmaType::SharedMemory {
                        let shmid = vma.file_fd();
                        if shmid >= 0 {
                            crate::ipc::sysv_shm::shm_detach_vma(shmid);
                        }
                    }
                }
            }
            let kernel_ppn = crate::arch::mm::mmu_init::root_page_table_ppn();
            let satp: u64;
            // SAFETY: plain CSR read of the current satp.
            unsafe { core::arch::asm!("csrr {}, satp", out(reg) satp) };
            let current_ppn = satp & 0xF_FFFF_FFFF_FFFF;
            if current_ppn != kernel_ppn {
                let new_satp = (8u64 << 60) | kernel_ppn;
                // SAFETY: switching to the kernel root page table; the
                // kernel linear mapping is present in every address space's
                // kernel portion and in ROOT_PAGE_TABLE itself.
                unsafe {
                    core::arch::asm!(
                        "csrw satp, {0}",
                        "sfence.vma zero, zero",
                        in(reg) new_satp,
                        options(nostack),
                    );
                }
            }
        }
        (*current).set_address_space(None);
        (*current).clear_active_mm();

        // Wake vfork parent (if any) — child has dropped the shared address
        // space, so the parent can safely resume.  This must happen before
        // ZOMBIE state so that the parent sees the child is still valid.
        crate::process::task::vfork_wake_parent(current);

        // ===== exit_files: Release file descriptor table reference =====
        (*current).set_fdtable(None);

        // ===== Clean up futex waiters =====
        crate::sync::futex::futex_cleanup(current);

        // ===== Clean up POSIX MQ fd entries =====
        crate::ipc::posix_mq::mq_fds_cleanup(current);

        // ===== Disarm interval timers (ITIMER_REAL/VIRTUAL/PROF) =====
        for i in 0..3 {
            let old_id = (*current).itimer_ids[i].swap(0, core::sync::atomic::Ordering::AcqRel);
            if old_id != 0 {
                crate::timer::del_timer(old_id);
            }
        }

        // ===== Disarm POSIX timers =====
        {
            let mut timers = (*current).posix_timers.lock();
            for pt in timers.drain(..) {
                if pt.kernel_timer_id != 0 {
                    crate::timer::del_timer(pt.kernel_timer_id);
                }
            }
        }

        // ===== Reverse SEM_UNDO adjustments =====
        crate::ipc::sysv_sem::sem_undo_exit(current);

        // ===== Orphan reparenting (review PROC-P03) =====
        // Children of a dying task must be re-attached (a surviving
        // thread-group member first, else the subreaper/init (PID 1)),
        // otherwise their parent pointers dangle after our Task slot is
        // freed (ppid()/for_each_child UAF) and orphaned ZOMBIEs are
        // never reaped — the PID space leaks monotonically.
        // Applies to threads too: a thread may have fork()ed children.
        // SAFETY: process-tree mutations take PROCESS_TREE_LOCK inside.
        unsafe {
            reparent_children_to_init(current);
        }

        // ===== Thread-group split =====
        if !is_leader {
            // ---- Non-leader thread: deferred release, never a ZOMBIE ----
            // Our mm Arc reference is already dropped above; other members
            // keep the address space alive.
            (*current).signal = None; // shared with the leader; drop our ref
            (*current).set_fs(None); // CLONE_FS-shared; drop our ref

            // Park on the leader's dead list BEFORE leaving the ring.
            // Ordering matters: nr_threads hits 1 in thread_group_leave,
            // which unblocks a dying leader (and the parent's reap of it).
            // If the push came after the leave, the leader could be fully
            // released in that window and we would push onto freed memory.
            // Parking first is safe: free_dead_member spins on on_cpu,
            // which stays set until our final schedule below — so the
            // leader's release cannot pass our entry while we still touch
            // leader fields.
            // SAFETY: leader comes from group_leader_ptr() while we are
            // still linked in its ring — it cannot be mid-release (its
            // release_task waits for ring members to be DEAD first).
            if !leader.is_null() {
                (*leader).dead_threads.lock().push(current);
            }

            // Leave the ring (decrements the leader's nr_threads) — AFTER
            // the mm drop and the dead-list park.
            let leader = Task::thread_group_leave(current);

            // PID + hash bookkeeping now: the tid disappears immediately
            // (gettid/tgkill on it → ESRCH, no zombie window).
            crate::process::pid_hash::pid_hash_remove(current_pid);
            crate::process::pid::free_pid(current_pid);

            // Opportunistically reclaim older dead members (frees stacks of
            // previously-joined threads; `current` is skipped — on_cpu).
            if !leader.is_null() {
                sweep_dead_threads(leader);
            }

            // R9-3 discipline: no preempt window between DEAD and the
            // final schedule.
            crate::interrupt::preempt::preempt_count_add(1);
            // Final state: DEAD (not ZOMBIE — wait4 never sees threads).
            (*current).set_state(TaskState::new(TaskState::DEAD));
            crate::sched::dequeue_task(&*current);
            crate::interrupt::preempt::preempt_count_sub(1);

            // ===== do_task_dead: Final schedule, never returns =====
            crate::sched::schedule();

            loop {
                // SAFETY: `wfi` is a plain hint instruction with no side effects.
                asm!("wfi", options(nomem, nostack));
            }
        }

        // ---- Leader (or single-threaded process): classic ZOMBIE flow ----
        // If group members are still alive (exit() from the leader, or
        // exit_group), kill them and wait for the drain before tearing
        // down the shared-mm-dependent state any further.
        if (*current).nr_threads() > 1 {
            kill_other_threads(current);
            wait_for_thread_group_death(current);
        }

        // ===== PID 1 exit: power the machine down =====
        // No orphan is ever reaped or respawned without init — the
        // shutdown(8) init-signal path lands here (init kills every user
        // process, including shutdown(8) itself, then exits). Runs the
        // reboot(2) cascade (never returns) unless one is already in
        // flight — that caller is waiting for exactly this exit.
        if current_pid == 1 {
            crate::syscall::process::init_exited_fallback_cascade();
        }

        // R9-3: close the preempt window between ZOMBIE and the deferred
        // notify — a timer IRQ landing here would schedule() us out with
        // state != RUNNING, and lines below (the notify arm) would never
        // run: the parent's only wake source lost.
        crate::interrupt::preempt::preempt_count_add(1);
        // Set process state to Zombie
        (*current).set_state(TaskState::new(TaskState::ZOMBIE));

        // Dequeue from global run queue
        crate::sched::dequeue_task(&*current);

        // ===== exit_notify: Defer parent notification =====
        // CRITICAL: Do NOT wake the parent here.  If the parent runs on
        // another CPU it can reap (free_task_slot) this task before
        // schedule() switches us away, causing a use-after-free that
        // corrupts ti_cpu (initialised to -1 in Task::new).
        // Instead, store the parent PID in a per-CPU deferred slot;
        // __schedule processes it AFTER the context switch, when this
        // task is no longer running on any CPU.
        if parent_pid != 0 {
            crate::sched::defer_exit_notify(parent_pid, current_pid);
        }
        crate::interrupt::preempt::preempt_count_sub(1);

        // ===== do_task_dead: Final schedule, never returns =====
        crate::sched::schedule();

        loop {
            // SAFETY: `wfi` is a plain hint instruction with no side effects.
            asm!("wfi", options(nomem, nostack));
        }
    }
}

/// exit_group — terminate the WHOLE thread group (Linux do_group_exit).
///
/// Force SIGKILL every other member (leader included, whoever calls it),
/// then run the caller's do_exit. Called from:
/// - sys_exit_group (NR 94)
/// - fatal default signal actions (Linux do_group_exit semantics: a fatal
///   signal in ANY thread kills the process)
pub fn do_exit_group(exit_code: i32) -> ! {
    let current = match crate::sched::current() {
        Some(c) => c as *mut Task,
        None => do_exit(exit_code),
    };
    kill_other_threads(current);
    do_exit(exit_code)
}

/// Move every child of `dying` to a new parent (Linux
/// forget_original_parent): the nearest PR_SET_CHILD_SUBREAPER ancestor if
/// one exists, else init (PID 1). Zombie children get a SIGCHLD to the new
/// parent so the shell's reaper loop can collect them; live children
/// simply change parent. Any child with a parent-death signal
/// (PR_SET_PDEATHSIG) registered receives it now. Runs before `dying`
/// transitions to ZOMBIE and under the process-tree lock, so no child can
/// observe a dangling parent.
///
/// # Safety
/// `dying` is the currently running task; child pointers come from its
/// children list and are protected by PROCESS_TREE_LOCK.
unsafe fn reparent_children_to_init(dying: *mut Task) {
    use crate::process::task::TaskState;

    // init task (PID 1); falls back to no-op when absent (early boot).
    // SAFETY: pid 1's Task lives until system shutdown.
    let init = crate::sched::find_task_by_pid(1);
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
    // A dying THREAD's children go to a surviving member of its own
    // thread group first (Linux find_new_reaper): the group's remaining
    // threads keep group-wide wait visibility, so wait4/waitid callers
    // in this process can still reap them. Only when no sibling
    // survives do we fall through to the subreaper/init walk.
    {
        let _lock = crate::process::task::PROCESS_TREE_LOCK.lock();
        let mut member = (*dying).next_thread_ptr();
        let mut guard = 0u32;
        while (member as *const Task) != (dying as *const Task)
            && !member.is_null()
            && guard < 65536
        {
            // Skip members that are already gone from the scheduler's
            // view; anything still linked in the ring is pre-leave and
            // therefore valid memory.
            let st = (*member).state();
            if !st.is_dead() && !st.contains(crate::process::task::TaskState::ZOMBIE) {
                // Pin INSIDE the lock hold: the member cannot leave the
                // ring (thread_group_leave takes this lock) and a pinned
                // member cannot be freed after leaving, so this fetch_add
                // can never land in freed memory.
                (*member)
                    .task_refcnt
                    .fetch_add(1, core::sync::atomic::Ordering::AcqRel);
                dest = member;
                dest_pin = member;
                break;
            }
            member = (*member).next_thread_ptr();
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
        let leader = (*dying).group_leader_ptr();
        let chain_root = if leader.is_null()
            || leader as *const Task == dying as *const Task
        {
            dying
        } else {
            leader as *mut Task
        };
        let mut p = (*chain_root).parent_ptr();
        let mut hops = 0;
        // The whole walk under the tree lock: every release_task of a
        // chain member has to pass remove_child under this lock first,
        // so no hop can be freed mid-walk.
        let _lock = crate::process::task::PROCESS_TREE_LOCK.lock();
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
            if pp == dying as *const Task as *mut Task || pp == init || hops > 64 {
                break;
            }
            let is_reaper = (*pp)
                .signal
                .as_ref()
                .map(|s| s.is_child_subreaper.load(core::sync::atomic::Ordering::Acquire))
                .unwrap_or(false);
            if is_reaper {
                dest = pp;
                // Pin the subreaper dest under the same lock hold (same
                // argument as the member pin above).
                (*pp)
                    .task_refcnt
                    .fetch_add(1, core::sync::atomic::Ordering::AcqRel);
                dest_pin = pp;
                break;
            }
            p = (*pp).parent_ptr();
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
    (*dying).collect_children_pinned(&mut moved);

    for child in moved {
        // E8-REPAR: atomic validate+unlink+relink. move_child skips a
        // child that a concurrent reaper already unlinked (claimed or
        // released) — after a false return the child may be freed memory
        // we must not touch beyond our own task_put.
        let ok = (*dying).move_child(child, dest);
        if !ok {
            Task::task_put(child);
            continue;
        }

        // PR_SET_PDEATHSIG: the old parent just died — deliver the signal
        // the child asked for (Linux: it fires on reparenting, before the
        // child could observe the orphaning).
        let pdeath = (*child).pdeath_signal;
        if pdeath != 0 {
            let _ = crate::signal::send_signal((*child).pid(), pdeath as i32);
        }

        // If the orphan is already a zombie, the new parent must be
        // notified so it reaps it — otherwise it would sit unreapable
        // forever.
        if (*child).state() == TaskState::new(TaskState::ZOMBIE) {
            let dest_pid = (*dest).pid();
            // Full do_notify_parent siginfo: the new parent's SA_SIGINFO
            // handlers decode the orphan zombie's outcome from
            // si_code/si_status.
            let mut info = crate::signal::SigInfo::child_exit(
                (*child).pid(),
                (*child).cred().uid,
                (*child).exit_code(),
                (*child).core_dumped(),
            );
            // do_notify_parent delivers the child's clone exit_signal.
            let orphan_sig = (*child).get_exit_signal();
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
            let dest_pinned = crate::process::pid_hash::pid_hash_lookup_pinned(dest_pid);
            if !dest_pinned.is_null() {
                // SAFETY: dest_pinned is pinned (refcount held).
                wake_group_chldexit(dest_pinned);
                crate::process::task::Task::task_put(dest_pinned);
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

// ============================================================================
// wait4 / waitpid / waitid
// ============================================================================

/// Linux's internal syscall-restart sentinel. Interrupted blocking waits
/// return this instead of -EINTR; the signal-delivery path converts it —
/// `setup_frame` restarts the ecall when the handler has SA_RESTART, and
/// `restart_syscall_no_handler` restarts it on the no-handler path, so
/// userspace only ever sees EINTR when the disposition really wants it.
/// wait4/waitid carry no timeout, so the rewind-and-reexecute restart is
/// always correct (glib's SIGCHLD handler is installed SA_RESTART; a bare
/// EINTR here broke g_spawn_sync-style waitpid loops).
const ERESTARTSYS: i32 = -512;

/// Wake every thread of `task`'s group on a child exit.
///
/// Linux hangs wait4/waitid off `signal->wait_chldexit`, a wait queue
/// SHARED by the whole thread group — any member blocked in a wait must
/// be woken when a child (of any member) exits. Our wait_chldexit queue
/// is per-Task, so the exit-notify paths must walk the ring and wake
/// each member's queue. The tree lock serializes ring joins/leaves, so
/// every member we touch is still linked (and hence not yet freed).
///
/// # Safety
/// `task` is a valid, pinned Task.
pub(crate) unsafe fn wake_group_chldexit(task: *mut Task) {
    use crate::process::task::PROCESS_TREE_LOCK;

    let _lock = PROCESS_TREE_LOCK.lock();
    let start = task;
    let mut member = task;
    let mut guard = 0u32;
    loop {
        let _ = (*member).wait_chldexit.wake_up_all();
        member = (*member).next_thread_ptr();
        if member == start || member.is_null() {
            break;
        }
        guard += 1;
        if guard > 65536 {
            break;
        }
    }
}

/// wait4 pid-selector semantics (Linux wait_tasktype):
/// - pid > 0 : that exact child
/// - pid == 0: any child in the CALLER's process group
/// - pid == -1: any child
/// - pid < -1: any child whose pgid == -pid
#[inline]
fn wait_pid_matches(child: &Task, pid: i32, caller_pgid: u32) -> bool {
    if pid > 0 {
        child.pid() == pid as u32
    } else if pid == 0 {
        child.pgid() == caller_pgid
    } else if pid == -1 {
        true
    } else {
        child.pgid() == (-pid) as u32
    }
}

/// Blocking wait for child process state change
///
/// # Arguments
/// * `pid` - PID to wait for (-1 = any child, >0 = specific PID,
///           0 = caller's process group, <-1 = process group -pid)
/// * `status_ptr` - User pointer to store exit status
/// * `options` - Wait options (WUNTRACED, etc.)
///
/// # Returns
/// * `Ok(child_pid)` - Child PID that changed state
/// * `Err(-ECHILD)` - No children
/// * `Err(-EINTR)` - Interrupted by signal
/// * `Err(-EFAULT)` - status_ptr unwritable
pub fn do_wait(pid: i32, status_ptr: *mut i32, options: i32) -> Result<Pid, i32> {

    // SAFETY: current is the calling task's raw pointer, valid throughout the wait loop.
    // The task sleeps via schedule() in INTERRUPTIBLE state so it won't be freed.
    unsafe {
        let current = match crate::sched::current() {
            Some(c) => c as *mut Task,
            None => return Err(errno::Errno::NoChild.as_neg_i32()),
        };

        let current_pid = (*current).pid();

        // If current is idle task (PID 0), no real process is running
        if current_pid == 0 {
            return Err(errno::Errno::NoChild.as_neg_i32());
        }

        let caller_pgid = (*current).pgid();

        loop {
            let mut found_child = false;
            let mut zombie_child: Option<*mut Task> = None;
            let mut stopped_child: Option<*mut Task> = None;

            const WUNTRACED: i32 = 0x00000002;

            // Iterate over the children of EVERY thread-group member
            // (Linux do_wait walks while_each_thread — any thread can
            // wait for any member's child; glib's gmain worker thread
            // reaps children the main thread spawned). Threads are never
            // on these lists — only fork()/clone-without-CLONE_THREAD
            // products are.
            (*current).for_each_group_child(|child_ptr| {
                let child = &*child_ptr;

                if !wait_pid_matches(child, pid, caller_pgid) {
                    return;
                }

                found_child = true;

                // Check if it's in Zombie state
                if child.state() == TaskState::new(TaskState::ZOMBIE) {
                    // E8-REPAR: CLAIM the zombie here, inside the same
                    // tree-lock critical section that selected it
                    // (unlink + parent=None). Two group members woken by
                    // the same SIGCHLD (glib reaper thread + main) could
                    // both select this zombie and double-release_task it
                    // — the second release would lock/unlink/free through
                    // the already-freed Task. Losers simply fail the
                    // claim and re-scan. Claim only the first winner:
                    // every claimed zombie must be reaped by this call or
                    // it becomes unreapable (no list links it anymore).
                    if zombie_child.is_none() && Task::claim_child_locked(child_ptr) {
                        zombie_child = Some(child_ptr);
                    }
                } else if child.state() == TaskState::new(TaskState::STOPPED)
                    && !child.stop_reported.load(core::sync::atomic::Ordering::Acquire)
                    && (options & WUNTRACED != 0 || child.tracer_pid() == current_pid)
                {
                    // Linux wait_task_stopped(): "Traditionally we see
                    // ptrace'd stopped children regardless of options" —
                    // WUNTRACED gates job-control stops only; a stop of a
                    // task WE trace is always reported (LTP ptrace01/05
                    // use plain waitpid on a TRACEME'd child).
                    stopped_child = Some(child_ptr);
                }
            });

            // If found zombie child, reap it
            if let Some(child_ptr) = zombie_child {
                let child = &*child_ptr;
                let child_pid = child.pid();
                let raw_exit = child.exit_code();

                // Encode exit status per waitpid ABI:
                // - Normal exit: status = (exit_code & 0xFF) << 8  (WIFEXITED, WEXITSTATUS)
                // - Killed by signal: status = |signal_number|      (WIFSIGNALED, WTERMSIG)
                //   plus WCOREDUMP (0x80) when a core file was written.
                let status: i32 = if raw_exit >= 0 {
                    (((raw_exit as u32) & 0xFF) << 8) as i32
                } else {
                    let mut s = (-(raw_exit as i32) as u32 & 0x7F) as i32;
                    if child.core_dumped() {
                        s |= 0x80; // WCOREDUMP
                    }
                    s
                };

                // Write exit status safely using copy_to_user
                if !status_ptr.is_null() {
                    let _uncopied = crate::arch::uaccess::copy_to_user(
                        status_ptr as *mut u8,
                        &status as *const i32 as *const u8,
                        core::mem::size_of::<i32>()
                    );
                }

                // Release task resources (kernel stack, Arc refs, PID)
                // Note: remove_child is done inside release_task()
                release_task(child_ptr);

                return Ok(child_pid);
            }

            // If found stopped child (WUNTRACED), report it
            if let Some(child_ptr) = stopped_child {
                let child = &*child_ptr;
                let child_pid = child.pid();
                let stop_sig = child.stop_signal();

                // Encode stopped status: (stop_signal << 8) | 0x7F
                let status: i32 = (((stop_sig as u32) << 8) | 0x7F) as i32;

                // Write status safely using copy_to_user
                if !status_ptr.is_null() {
                    let _uncopied = crate::arch::uaccess::copy_to_user(
                        status_ptr as *mut u8,
                        &status as *const i32 as *const u8,
                        core::mem::size_of::<i32>()
                    );
                }

                // Mark this stop event as reported (only report once)
                child.stop_reported.store(true, core::sync::atomic::Ordering::Release);

                // Note: stopped child is NOT reaped, it stays in children list
                return Ok(child_pid);
            }

            // Linux do_wait() also walks the caller's TRACEE list
            // (ptrace_do_wait): a task attached with PTRACE_ATTACH that is
            // not a real child (LTP ptrace11 attaches init and
            // waitpid(1,...)s) is still waitable for its stops. A tracee
            // that has not stopped yet also keeps the wait blocking (it
            // plays the role of a child for ECHILD purposes).
            let mut found_tracee = false;
            {
                let mut tracee_stop: Option<*mut Task> = None;
                crate::process::pid_hash::pid_hash_for_each_task(|t| unsafe {
                    if tracee_stop.is_some() {
                        return;
                    }
                    if (*t).tracer_pid() != current_pid || (*t).pid() == current_pid {
                        return;
                    }
                    if !wait_pid_matches(&*t, pid, caller_pgid) {
                        return;
                    }
                    found_tracee = true;
                    if (*t).state() == TaskState::new(TaskState::STOPPED)
                        && !(*t).stop_reported.load(core::sync::atomic::Ordering::Acquire)
                    {
                        tracee_stop = Some(t);
                    }
                });
                if let Some(t) = tracee_stop {
                    let child_pid = (*t).pid();
                    let stop_sig = (*t).stop_signal();
                    let status: i32 = (((stop_sig as u32) << 8) | 0x7F) as i32;
                    if !status_ptr.is_null() {
                        let _uncopied = crate::arch::uaccess::copy_to_user(
                            status_ptr as *mut u8,
                            &status as *const i32 as *const u8,
                            core::mem::size_of::<i32>()
                        );
                    }
                    (*t).stop_reported.store(true, core::sync::atomic::Ordering::Release);
                    return Ok(child_pid);
                }
            }

            // No zombie or stopped child found
            if found_child || found_tracee {
                // Atomically add to waitqueue AND set INTERRUPTIBLE.
                // Prevents lost-wakeup race where child exits between
                // add() and set_state(), marking the entry woken but
                // wake_up_process skips the actual wake (task still RUNNING).
                (*current).wait_chldexit.prepare_to_wait(current, false, true);

                // Recheck after prepare_to_wait (waker may have fired
                // between the initial scan and prepare_to_wait — a wake of
                // a still-RUNNING waiter is refused, so the state change
                // can only be caught here). If a child is already zombie or
                // freshly STOPPED (ptrace stop / group stop landing in that
                // window), finish_wait restores RUNNING state and we loop
                // to report it — no schedule() needed. Without the stopped
                // recheck, a stop that raced the arm was only visible via
                // the (discarded-for-SIG_IGN-SIGCHLD) signal path and the
                // wait slept forever (LTP ptrace05 wedge family).
                {
                    let mut need_rescan = false;
                    (*current).for_each_group_child(|child_ptr| {
                        if need_rescan {
                            return;
                        }
                        if !wait_pid_matches(&*child_ptr, pid, caller_pgid) {
                            return;
                        }
                        let st = (*child_ptr).state();
                        if st == TaskState::new(TaskState::ZOMBIE) {
                            need_rescan = true;
                        } else if st == TaskState::new(TaskState::STOPPED)
                            && !(*child_ptr).stop_reported.load(core::sync::atomic::Ordering::Acquire)
                            && (options & WUNTRACED != 0
                                || (*child_ptr).tracer_pid() == current_pid)
                        {
                            need_rescan = true;
                        }
                    });
                    if !need_rescan {
                        crate::process::pid_hash::pid_hash_for_each_task(|t| unsafe {
                            if need_rescan {
                                return;
                            }
                            if (*t).tracer_pid() != current_pid || (*t).pid() == current_pid {
                                return;
                            }
                            if !wait_pid_matches(&*t, pid, caller_pgid) {
                                return;
                            }
                            if (*t).state() == TaskState::new(TaskState::STOPPED)
                                && !(*t).stop_reported.load(core::sync::atomic::Ordering::Acquire)
                            {
                                need_rescan = true;
                            }
                        });
                    }
                    if need_rescan {
                        (*current).wait_chldexit.finish_wait(current);
                        // The child's wake_up() may have raced our
                        // prepare_to_wait and enqueued us; we are going to
                        // keep running on this CPU — take ourselves back
                        // off the queue (review NEW-C2).
                        crate::sched::dequeue_if_enqueued(&*current);
                        continue; // re-enter loop to report/reap
                    }
                }

                // Check for pending signals before sleeping
                use crate::signal;
                if signal::signal_pending() {
                    (*current).wait_chldexit.finish_wait(current);
                    crate::sched::dequeue_if_enqueued(&*current);
                    // ERESTARTSYS, not EINTR: the signal-delivery path
                    // restarts the wait when the handler is SA_RESTART
                    // (Linux semantics — glib's SIGCHLD handler is).
                    return Err(ERESTARTSYS);
                }

                // Enable interrupts before schedule(). We're in syscall context
                // (SIE=0). Without this, timer IRQ can't fire and the task
                // can never be rescheduled.
                crate::arch::cpu::restore_irq(true);

                // Schedule other processes
                crate::sched::schedule();

                // After wakeup: state is RUNNING (set by wake_up_process).
                // Clean up waitqueue entry before next iteration.
                (*current).wait_chldexit.finish_wait(current);
            } else {
                // No child processes at all
                return Err(errno::Errno::NoChild.as_neg_i32());
            }
        }
    }
}

/// Non-blocking wait for child process
///
/// # Arguments
/// * `pid` - PID to wait for (-1 = any, >0 = specific, 0 = caller's pgid,
///           <-1 = pgid == -pid)
/// * `status_ptr` - User pointer to store exit status
/// * `options` - WUNTRACED (report stopped children too)
///
/// # Returns
/// * `Ok(child_pid)` - Child PID that has exited / was reported stopped
/// * `Err(-ECHILD)` - No children
/// * `Err(-EAGAIN)` - Children exist but none reported (sys_wait4 → 0)
pub fn do_wait_nonblock(pid: i32, status_ptr: *mut i32, options: i32) -> Result<Pid, i32> {
    const WUNTRACED: i32 = 0x00000002;

    // SAFETY: current is the calling task's raw pointer, valid throughout this call.
    unsafe {
        let current = match crate::sched::current() {
            Some(c) => c as *mut Task,
            None => return Err(errno::Errno::NoChild.as_neg_i32()),
        };

        let current_pid = (*current).pid();

        // If current is idle task (PID 0), no real process is running
        if current_pid == 0 {
            return Err(errno::Errno::NoChild.as_neg_i32());
        }

        let caller_pgid = (*current).pgid();

        let mut found_child = false;
        let mut zombie_ptr: Option<*mut Task> = None;
        let mut stopped_ptr: Option<*mut Task> = None;

        // Scan the children of every thread-group member to find a
        // zombie / WUNTRACED stop — do NOT modify the list during
        // iteration. (Group-wide: glib's gmain worker thread polls
        // waitpid(WNOHANG) for children the main thread spawned.)
        (*current).for_each_group_child(|child_ptr| {
            let child = &*child_ptr;

            if !wait_pid_matches(child, pid, caller_pgid) {
                return;
            }

            found_child = true;

            if child.state() == TaskState::new(TaskState::ZOMBIE) && zombie_ptr.is_none() {
                zombie_ptr = Some(child_ptr);
            } else if child.state() == TaskState::new(TaskState::STOPPED)
                && !child.stop_reported.load(core::sync::atomic::Ordering::Acquire)
                && (options & WUNTRACED != 0 || child.tracer_pid() == current_pid)
                && stopped_ptr.is_none()
            {
                // ptrace'd stops are reported regardless of WUNTRACED
                // (Linux wait_task_stopped); see do_wait for the rationale.
                stopped_ptr = Some(child_ptr);
            }
        });

        if let Some(child_ptr) = zombie_ptr {
            let child = &*child_ptr;
            let child_pid = child.pid();
            let raw_exit = child.exit_code();

            let status: i32 = if raw_exit >= 0 {
                (((raw_exit as u32) & 0xFF) << 8) as i32
            } else {
                let mut s = (-(raw_exit as i32) as u32 & 0x7F) as i32;
                if child.core_dumped() {
                    s |= 0x80; // WCOREDUMP
                }
                s
            };

            if !status_ptr.is_null() {
                if crate::arch::uaccess::copy_to_user(
                    status_ptr as *mut u8,
                    &status as *const i32 as *const u8,
                    core::mem::size_of::<i32>(),
                ) != 0 {
                    return Err(crate::errno::Errno::BadAddress.as_neg_i32());
                }
            }

            release_task(child_ptr);
            return Ok(child_pid);
        }

        if let Some(child_ptr) = stopped_ptr {
            let child = &*child_ptr;
            let child_pid = child.pid();
            let stop_sig = child.stop_signal();
            let status: i32 = (((stop_sig as u32) << 8) | 0x7F) as i32;
            if !status_ptr.is_null() {
                if crate::arch::uaccess::copy_to_user(
                    status_ptr as *mut u8,
                    &status as *const i32 as *const u8,
                    core::mem::size_of::<i32>(),
                ) != 0 {
                    return Err(crate::errno::Errno::BadAddress.as_neg_i32());
                }
            }
            child.stop_reported.store(true, core::sync::atomic::Ordering::Release);
            return Ok(child_pid);
        }

        if found_child {
            Err(errno::Errno::TryAgain.as_neg_i32())
        } else {
            Err(errno::Errno::NoChild.as_neg_i32())
        }
    }
}

/// waitid options (match Linux wait.h)
const WNOHANG: i32     = 0x00000001;
const WUNTRACED: i32   = 0x00000002;
const WSTOPPED: i32    = WUNTRACED;
const WEXITED: i32     = 0x00000004;
const WCONTINUED: i32  = 0x00000008;
const WNOWAIT: i32     = 0x01000000;

/// waitid idtype
const P_ALL: i32  = 0;
const P_PID: i32  = 1;
const P_PGID: i32 = 2;

/// CLD_* si_code values for siginfo_t
const CLD_EXITED: i32    = 1;
const CLD_KILLED: i32    = 2;
const CLD_DUMPED: i32    = 3;
const CLD_STOPPED: i32   = 5;
const CLD_CONTINUED: i32 = 6;

/// Write waitid siginfo_t fields to user memory.
///
/// Only writes the fields used by waitid (si_signo, si_errno, si_code,
/// si_pid, si_uid, si_status), leaving the rest of the 128-byte
/// siginfo_t untouched.
// SAFETY: Caller must ensure infop points to a valid, writable user buffer of at
// least 128 bytes. copy_to_user handles user-space access safely.
unsafe fn write_siginfo(
    infop: *mut u8,
    si_code: i32,
    si_pid: u32,
    si_uid: u32,
    si_status: i32,
) {
    use crate::arch::uaccess::copy_to_user;
    let base = infop;

    let signo = 17i32; // SIGCHLD
    let _ = copy_to_user(base, &signo as *const i32 as *const u8, 4);
    let _ = copy_to_user(base.add(4), &0i32 as *const i32 as *const u8, 4);
    let _ = copy_to_user(base.add(8), &si_code as *const i32 as *const u8, 4);
    // offset 12: padding
    let _ = copy_to_user(base.add(16), &si_pid as *const u32 as *const u8, 4);
    let _ = copy_to_user(base.add(20), &si_uid as *const u32 as *const u8, 4);
    let _ = copy_to_user(base.add(24), &si_status as *const i32 as *const u8, 4);
}

/// Blocking waitid: wait for child process state change
///
/// # Arguments
/// * `idtype` - P_ALL (0), P_PID (1), or P_PGID (2)
/// * `id` - PID or PGID to wait for (ignored if P_ALL)
/// * `infop` - User pointer to siginfo_t
/// * `options` - WNOHANG | WEXITED | WSTOPPED | WCONTINUED | WNOWAIT
///
/// # Returns
/// * `Ok(true)` - Event found (child info written to infop)
/// * `Ok(false)` - WNOHANG and no event yet: return 0 to userspace and
///   leave infop UNTOUCHED (Linux/POSIX behavior — the old -EAGAIN broke
///   every WNOHANG poll loop).
/// * `Err(-ECHILD)` - No children
/// * `Err(-EINTR)` - Interrupted by signal
pub fn do_waitid(
    idtype: i32,
    id: i32,
    infop: *mut u8,
    options: i32,
) -> Result<bool, i32> {

    // Must specify at least one of WEXITED/WSTOPPED/WCONTINUED
    if options & (WEXITED | WSTOPPED | WCONTINUED) == 0 {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }

    // SAFETY: current is the calling task's raw pointer, valid throughout the wait loop.
    unsafe {
        let current = match crate::sched::current() {
            Some(c) => c as *mut Task,
            None => return Err(errno::Errno::NoChild.as_neg_i32()),
        };

        if (*current).pid() == 0 {
            return Err(errno::Errno::NoChild.as_neg_i32());
        }

        loop {
            let mut found_child = false;
            let mut result_child: Option<*mut Task> = None;
            let mut result_code: i32 = 0;
            let mut result_kind: i32 = 0; // 0=zombie, 1=stopped, 2=continued

            // Group-wide scan (same as do_wait): any thread of the
            // process may waitid for any member's child.
            (*current).for_each_group_child(|child_ptr| {
                let child = &*child_ptr;

                // idtype filter
                if idtype == P_PID && child.pid() != id as u32 {
                    return;
                }
                if idtype == P_PGID && child.pgid() != id as u32 {
                    return;
                }
                // P_ALL: match any child

                found_child = true;

                // Check for zombie (exited) — only if WEXITED
                if options & WEXITED != 0
                    && child.state() == TaskState::new(TaskState::ZOMBIE)
                {
                    result_child = Some(child_ptr);
                    let raw_exit = child.exit_code();
                    let mut was_killed = false;
                    if raw_exit >= 0 {
                        result_code = raw_exit as i32;
                    } else {
                        result_code = (-raw_exit) as i32;
                        was_killed = true;
                    }
                    result_kind = if was_killed { 3 } else { 0 };
                }
                // Check for stopped — only if WSTOPPED. Unified with
                // wait4's one-shot semantics: stop_reported, not a
                // separate stop_signal()!=0 test (the two mechanisms
                // racing double-reported one stop event — review 3.3).
                else if options & WSTOPPED != 0
                    && child.state() == TaskState::new(TaskState::STOPPED)
                {
                    if !child.stop_reported.load(core::sync::atomic::Ordering::Acquire) {
                        result_child = Some(child_ptr);
                        result_code = child.stop_signal();
                        result_kind = 1; // stopped
                    }
                }
                // Check for continued — only if WCONTINUED
                else if options & WCONTINUED != 0 {
                    // A continued child would have been STOPPED with
                    // stop_signal == 0 (cleared by SIGCONT handler).
                    // For now this is a future enhancement.
                }
            });

            if let Some(child_ptr) = result_child {
                let child = &*child_ptr;
                let child_pid = child.pid();
                let child_uid = child.cred().uid;

                // Encode si_code and si_status
                let (si_code, si_status) = if result_kind == 1 {
                    // Stopped
                    (CLD_STOPPED, result_code)
                } else if result_kind == 3 {
                    // Killed by signal (CLD_DUMPED when a core was written)
                    if child.core_dumped() {
                        (CLD_DUMPED, result_code)
                    } else {
                        (CLD_KILLED, result_code)
                    }
                } else {
                    // Normal exit
                    (CLD_EXITED, result_code)
                };

                // Write siginfo to user
                write_siginfo(infop, si_code, child_pid, child_uid, si_status);

                // Reap zombie unless WNOWAIT
                if (result_kind == 0 || result_kind == 3) && options & WNOWAIT == 0 {
                    release_task(child_ptr);
                }

                // Mark the stop event reported (one-shot, wait4-consistent)
                if result_kind == 1 && options & WNOWAIT == 0 {
                    (*child_ptr).stop_reported.store(true, core::sync::atomic::Ordering::Release);
                }

                return Ok(true);
            }

            // No matching child found
            if found_child {
                // Children exist but none in target state
                if options & WNOHANG != 0 {
                    return Ok(false);
                }

                // Atomically add to waitqueue AND set INTERRUPTIBLE.
                (*current).wait_chldexit.prepare_to_wait(current, false, true);

                // R9-10: re-check for a zombie AFTER registration (do_wait
                // has this; the child's deferred notify may have fired
                // while we were still RUNNING, consuming the only wake).
                // R20-4: the recheck MUST apply the same idtype/id filter
                // as the main scan — without it, a zombie sibling that the
                // filter excludes (e.g. waitid(P_PID, X) while Y is zombie)
                // keeps setting found_zombie, and the loop takes the
                // `continue` path forever without ever sleeping: a
                // kernel-space busy hang.
                {
                    let mut found_zombie = false;
                    (*current).for_each_group_child(|child_ptr| {
                        if idtype == P_PID && (*child_ptr).pid() != id as u32 {
                            return;
                        }
                        if idtype == P_PGID && (*child_ptr).pgid() != id as u32 {
                            return;
                        }
                        if (*child_ptr).state() == TaskState::new(TaskState::ZOMBIE) {
                            found_zombie = true;
                        }
                    });
                    if found_zombie {
                        (*current).wait_chldexit.finish_wait(current);
                        crate::sched::dequeue_if_enqueued(&*current);
                        continue;
                    }
                }

                if crate::signal::signal_pending() {
                    (*current).wait_chldexit.finish_wait(current);
                    // R8-5 (NEW-C2): undo the signal's concurrent enqueue.
                    crate::sched::dequeue_task(&*current);
                    // ERESTARTSYS, not EINTR: the signal-delivery path
                    // restarts the wait when the handler is SA_RESTART.
                    return Err(ERESTARTSYS);
                }

                // Enable interrupts before schedule() — we're in syscall
                // context (SIE=0). Without this, timer IRQ can't fire and
                // the task can never be rescheduled.
                crate::arch::cpu::restore_irq(true);

                crate::sched::schedule();

                // After wakeup: state is RUNNING (set by wake_up_process).
                (*current).wait_chldexit.finish_wait(current);
            } else {
                return Err(errno::Errno::NoChild.as_neg_i32());
            }
        }
    }
}
