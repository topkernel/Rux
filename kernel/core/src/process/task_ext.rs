//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! THE TASK SERVICE BLOCK (rung F1, seed-core-design §4): the cold
//! field families that moved out of `Task` behind ONE bridge field
//! (`Task::ext: TaskExtHandle`). First family landed: the statistics
//! and hunt counters — the per-task forensic rings (LASTSYS,
//! last_fails, mm_ops, last_calls/init_calls/birth_calls), the
//! accounting scalars (cutime/cstime, start_jiffies, cmaxrss,
//! exit_maxrss) and did_exec.
//!
//! Placement discipline (binding, verified per field before the
//! move): a field lives here ONLY when no hot path — context switch,
//! trap entry/exit accounting, scheduler pick/walk, page fault, VFS
//! namei — reads or writes it directly. The rings' writers are
//! syscall-dispatch arms gated behind `rux:sysforensic`/`LASTSYS`
//! statics (quiet boots pay one atomic load, unchanged); mm_ops is
//! written only on the six address-space syscalls and read by the
//! panic-time fault printers; the accounting scalars are reaped at
//! wait4/exit and read by procfs. utime/stime (tick accounting),
//! nvcsw/nivcsw (the per-switch note_switch writes), min_flt/maj_flt
//! (page-fault path), schedstat_* (the __schedule edges) and the
//! pick/wake/wait counters (entities.rs) are DIRECT hot-path writes
//! and therefore stayed in the core block.
//!
//! INV-F1 (the bridge lifecycle): `Task::ext` is allocated at task
//! creation — `Task::new` constructs the `TaskExt` in the same breath
//! as the struct literal, so every constructor route (`new_task_at`,
//! `new_idle_at`, the init task's placement) yields a Task whose ext
//! is non-null — and freed at slot release: `free_task_slot`'s
//! `drop_in_place` drops the Box (the FTS-LEAK branch withholds the
//! page and the ext with it, exactly as for every other Task field).
//! The slot lifecycle therefore owns BOTH blocks: no path may free
//! one without the other, and no path may dereference a Task's ext
//! after its slot has been released. If the bridge ever shows up in
//! a switch-path profile, the field that pulled it in moves back
//! into the core block — the split narrows, never widens the hot
//! surface.
//!
//! Layout note: `TaskExt` is `#[repr(C)]` so any future framework
//! side that must reach a specific field without naming the type can
//! do so under a fixed-offset contract (the journal_seam
//! INV-JSEAM-1 precedent); today no framework reader needs one — all
//! access is through the typed handle. The type is framework-defined
//! for now; it flips into the process service crate together
//! with the namespace family (the last family), whose ns.rs types
//! move in the same step per the F1 design.

#![forbid(unsafe_code)]
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize};

use super::task::{BIRTH_CALLS_LEN, INIT_CALLS_LEN, LAST_CALLS_LEN};

// TaskExt, TaskExtHandle — the definitions live in process/types.rs; every impl and
// function that uses them stays in this file. Re-exported so the
// existing `process::task_ext::…` paths are unchanged.
pub use super::types::{TaskExt, TaskExtHandle};

impl TaskExt {
    pub fn new() -> Self {
        TaskExt {
            last_syscall_nr: AtomicU32::new(0),
            last_syscall_a0: AtomicU64::new(0),
            last_syscall_a1: AtomicU64::new(0),
            last_fails: [const { AtomicU32::new(0) }; 8],
            last_fails_cur: AtomicUsize::new(0),
            last_fails_a0: [const { AtomicU64::new(0) }; 8],
            last_fails_a1: [const { AtomicU64::new(0) }; 8],
            mm_ops: [const { AtomicU64::new(0) }; 32],
            mm_ops_cur: AtomicUsize::new(0),
            did_exec: AtomicBool::new(false),
            last_calls: [const { AtomicU32::new(0) }; LAST_CALLS_LEN],
            last_calls_cur: AtomicUsize::new(0),
            init_calls: [const { AtomicU32::new(0) }; INIT_CALLS_LEN],
            init_a0: [const { AtomicU64::new(0) }; INIT_CALLS_LEN],
            init_calls_cur: AtomicUsize::new(0),
            birth_calls: [const { AtomicU32::new(0) }; BIRTH_CALLS_LEN],
            birth_a0: [const { AtomicU64::new(0) }; BIRTH_CALLS_LEN],
            birth_done: AtomicBool::new(false),
            cutime_ticks: AtomicU64::new(0),
            cstime_ticks: AtomicU64::new(0),
            // Same stamp semantics as the old in-Task field: the
            // creation moment of THIS task (fork children get their
            // own stamp, as before).
            start_jiffies: AtomicU64::new(crate::timer::get_jiffies()),
            cmaxrss_kb: AtomicU64::new(0),
            exit_maxrss_kb: AtomicU64::new(0),
            tracer_pid: AtomicU32::new(0),
            ptrace_options: AtomicU64::new(0),
            ptrace_sigdeliver: AtomicU32::new(0),
            ptrace_siginfo: crate::sync::spinlock::Spinlock::new(None),
            ss_addr: AtomicU64::new(0),
            ss_active: AtomicBool::new(false),
            ss_saved_insn: AtomicU64::new(0),
            core_dumped: AtomicBool::new(false),
            clear_child_tid: core::ptr::null_mut(),
            robust_list_head: core::ptr::null(),
            robust_list_len: 0,
            futex_wait_enqueued: AtomicBool::new(false),
            sigmask_restore: 0,
            sigmask_restore_valid: false,
        }
    }
}

impl TaskExtHandle {
    #[inline]
    pub fn new() -> Self {
        TaskExtHandle(alloc::boxed::Box::new(TaskExt::new()))
    }
}

impl Deref for TaskExtHandle {
    type Target = TaskExt;
    #[inline]
    fn deref(&self) -> &TaskExt {
        &*self.0
    }
}

impl DerefMut for TaskExtHandle {
    #[inline]
    fn deref_mut(&mut self) -> &mut TaskExt {
        &mut *self.0
    }
}
