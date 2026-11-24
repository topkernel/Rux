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
//! for now; it flips into the rux-process service crate together
//! with the namespace family (the last family), whose ns.rs types
//! move in the same step per the F1 design.

use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize};

use super::task::{BIRTH_CALLS_LEN, INIT_CALLS_LEN, LAST_CALLS_LEN};

/// The Task service block: cold field families behind the bridge.
///
/// Field order mirrors the family order they arrived in; none of
/// these are read on a hot path (see the module doc's placement
/// discipline).
#[repr(C)]
pub struct TaskExt {
    // ---- family (a): statistics / hunt counters ----

    /// LASTSYS (stall hunts): the syscall this task most recently
    /// ENTERED (nr + first two args); names the call a parked task is
    /// sleeping in. Relaxed stores at dispatch entry; read by the
    /// DEBUG taskdump and the exit printer.
    pub last_syscall_nr: AtomicU32,
    pub last_syscall_a0: AtomicU64,
    pub last_syscall_a1: AtomicU64,

    /// sc125 (Session P): ring of the last 8 FAILING syscalls (nr
    /// packed low-16, errno high-16) — the pre-park failure tail.
    pub last_fails: [AtomicU32; 8],
    pub last_fails_cur: AtomicUsize,
    pub last_fails_a0: [AtomicU64; 8],
    pub last_fails_a1: [AtomicU64; 8],

    /// 50th (crash-storm hunt): per-task ring of address-space edits
    /// for the USEGV forensics (see the dispatch-side writer for the
    /// encoding). UNGATED by design — the fault printer must see these
    /// on quiet boots; the writer pays a handful of Relaxed atomics on
    /// the six address-space syscalls only.
    pub mm_ops: [AtomicU64; 32],
    pub mm_ops_cur: AtomicUsize,

    /// 50th: latch of the first execve (failed-spawn forensics, read
    /// by the do_exit printer).
    pub did_exec: AtomicBool,

    /// sc128/sc135 (Session P/Q): ring of the last syscalls
    /// (successes included) — the park decode's call tail.
    pub last_calls: [AtomicU32; LAST_CALLS_LEN],
    pub last_calls_cur: AtomicUsize,

    /// sc138 (Session Q): FILTERED init ring (see task.rs's INIT_CALLS_LEN
    /// doc for the skip list) — the byte-diff substrate for
    /// working-vs-parked children.
    pub init_calls: [AtomicU32; INIT_CALLS_LEN],
    pub init_a0: [AtomicU64; INIT_CALLS_LEN],
    pub init_calls_cur: AtomicUsize,

    /// sc143: one-shot latch of the task's FIRST 64 filtered calls
    /// (the birth window) that survives later ring churn.
    pub birth_calls: [AtomicU32; BIRTH_CALLS_LEN],
    pub birth_a0: [AtomicU64; BIRTH_CALLS_LEN],
    pub birth_done: AtomicBool,

    /// Reaped-children CPU accounting (folded at wait4; read by
    /// procfs /proc/[pid]/stat).
    pub cutime_ticks: AtomicU64,
    pub cstime_ticks: AtomicU64,

    /// Jiffies stamp of task creation (procfs starttime; reap
    /// uptime math).
    pub start_jiffies: AtomicU64,

    /// Max-RSS accounting: the value the parent folds in at reap
    /// (getrusage RUSAGE_CHILDREN; LTP getrusage03).
    pub cmaxrss_kb: AtomicU64,
    /// The dying task's own final maxrss (set at exit).
    pub exit_maxrss_kb: AtomicU64,

    // ---- family (b): ptrace state ----
    // Framework touchpoints are all cold and accessor-routed: the
    // trap files' int3/single-step arms (debugger-driven only),
    // signal delivery's traced-task interception (runs only when
    // tracer_pid != 0), the exit printer, and the reap-side
    // core_dumped check. Task keeps forwarding accessors.

    /// PTRACE tracer pid (0 = untraced). Read by signal delivery's
    /// interception check and the trap files' int3 handling.
    pub tracer_pid: AtomicU32,

    /// PTRACE_SETOPTIONS bits (crate-side writer via the Task
    /// accessor).
    pub ptrace_options: AtomicU64,

    /// The signal re-injected by PTRACE_CONT(sig) — delivered for
    /// real instead of intercepted (the peek/consume pair).
    pub ptrace_sigdeliver: AtomicU32,

    /// The dequeued signal held for the tracer (PTRACE_GETSIGINFO /
    /// CONT(0) swallows it).
    pub ptrace_siginfo: crate::sync::spinlock::Spinlock<
        Option<crate::signal::SigInfo>,
    >,

    /// Software single-step state (the x86 int3/stepping arm and the
    /// riscv64 epc-match arm; armed/cleared by ptrace).
    pub ss_addr: AtomicU64,
    pub ss_active: AtomicBool,
    pub ss_saved_insn: AtomicU64,

    /// A core dump was already attempted for this task (the reap-side
    /// check keeps signal death from re-dumping).
    pub core_dumped: AtomicBool,
}

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
        }
    }
}

/// The typed bridge handle (the journal_handle precedent, minus the
/// type erasure: the pointee is framework-typed until the namespace
/// family flips this block into the service crate). INV-F1 (see the
/// module doc) governs its lifetime: allocated at task creation,
/// freed at slot release, owned by the slot lifecycle.
pub struct TaskExtHandle(alloc::boxed::Box<TaskExt>);

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
