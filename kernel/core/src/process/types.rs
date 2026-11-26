//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The process subsystem's type definitions (seed-core-design §2c.3).
//!
//! Definitions only. Every `struct`, `enum` and type alias of the
//! subsystem lives here, together with the constants those definitions
//! name (array lengths, and the constants those derive from); every
//! `impl` block lives in `hotpath.rs` beside the functions that use it —
//! an impl is code, a definition is data layout. The exceptions are
//! definitions local to code: a type declared inside a function or
//! inside a private inline module of `hotpath.rs`.
//!
//! `TaskFlags` is the one definition that brings impl blocks with it:
//! the `bitflags!` invocation generates the struct and its operations
//! from a single macro call, as a `#[derive]` attribute does, so the
//! invocation moved whole.
//!
//! This file holds no `unsafe`. A field that was private to its old
//! topic file is `pub(super)` here because the code that touches it
//! lives in the sibling module `hotpath`: the process directory is the
//! smallest visibility boundary Rust offers for that split.

#![forbid(unsafe_code)]

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use bitflags::bitflags;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize};

use crate::fs::FdTable;
use crate::list::ListHead;
use crate::mm::pagemap::AddressSpace;
use crate::signal::{SignalStruct, SigPending};
use crate::sync::spinlock::Spinlock;

// ---------------------------------------------------------------------------
// the task control block: Task with its state, policy, flags, pid and
// credentials, and the definitions Task names — the stack-cache records,
// the BPF filter instruction, the POSIX timer state, the ring lengths
// and the rlimit count
// ---------------------------------------------------------------------------

/// Free stack entries (linked list)
///
/// Each entry is stack_bottom, we store next pointer at stack_bottom
/// This works because stack_bottom is unused (stack grows down from top)
pub(crate) struct FreeStackEntry {
    pub(crate) next: *mut FreeStackEntry,
}

/// Stack cache structure (the Send impl lives in unsafe.rs)
pub(crate) struct StackCache {
    /// Number of cached stacks
    pub(super) count: usize,
    /// Head of free stack list
    pub(super) head: *mut FreeStackEntry,
}

#[cfg(target_arch = "x86_64")]
pub(crate) struct KstackBlocks {
    pub(super) bases: alloc::vec::Vec<*mut u8>,
    pub(super) maps: alloc::vec::Vec<u8>,
}

/// DEBUG (LTP chunk@700 hunt, 2026-11-02): ring of recently freed kernel
/// stacks, so a kernel fault with rsp inside a freed-but-recycled stack
/// names the freer. Diagnostic only; cold path.
pub struct FreedStackRec {
    pub bottom: usize,
    pub top: usize,
    pub pid: u32,
    pub free_jiffies: u64,
}

/// Process state flags (bitmap form)
///
/// Bitmap representation for process states, allowing combined states
/// e.g.: TASK_UNINTERRUPTIBLE | __TASK_STOPPED
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TaskState(pub(super) u32);

///
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum SchedPolicy {
    /// Normal time-sharing scheduling (SCHED_NORMAL)
    Normal = 0,

    /// FIFO real-time scheduling (SCHED_FIFO)
    Fifo = 1,

    /// RR real-time scheduling (SCHED_RR)
    Rr = 2,

    /// Batch scheduling (SCHED_BATCH)
    Batch = 3,

    /// Idle scheduling (SCHED_IDLE)
    Idle = 5,

    /// Deadline scheduling (SCHED_DEADLINE)
    Deadline = 6,
}

bitflags! {
    pub struct TaskFlags: u32 {
        const PF_KTHREAD     = 0x00200000; /* I am a kernel thread */
        const PF_EXITING     = 0x00000004; /* Getting shut down */
        const PF_VCPU        = 0x00000010; /* I'm a virtual CPU */
        const PF_WQ_WORKER   = 0x00000020; /* I'm a workqueue worker */
        /* Kernel-preemption recursion guard (sched::schedule sets it
         * around __schedule; the IRQ-exit hook refuses to re-enter a
         * task that is already inside its own schedule). A task-flag,
         * deliberately NOT a preempt_count bit: leaked count bits
         * block both preemption AND the __schedule switch-out
         * discipline, while a leaked flag here only disables
         * preemption for the task. */
        const TIF_IN_SCHED   = 0x00400000;
    }
}

/// Process identifier (PID type)
///
pub type Pid = u32;

/// Process credentials.
#[derive(Debug, Clone)]
#[repr(C)]
pub struct Cred {
    pub uid: u32,
    pub gid: u32,
    pub euid: u32,
    pub egid: u32,
    pub suid: u32,
    pub sgid: u32,
    pub fsuid: u32,
    pub fsgid: u32,
    /// Supplementary group IDs
    pub groups: alloc::vec::Vec<u32>,
    // Capability sets (POSIX.1e)
    pub cap_inheritable: crate::security::capability::Cap,
    pub cap_permitted:   crate::security::capability::Cap,
    pub cap_effective:   crate::security::capability::Cap,
    pub cap_bounding:    crate::security::capability::Cap,
    pub cap_ambient:     crate::security::capability::Cap,
    /// PR_SET_KEEPCAPS: keep capability sets across a uid transition away
    /// from 0 (libcap-ng's capng_change_id() sets this before dropping to
    /// the daemon user, then clears it).
    pub keepcaps: bool,
    /// PR_SET_SECUREBITS bitmask (SECBIT_*): tracked for prctl parity;
    /// SECBIT_KEEP_CAPS mirrors `keepcaps`. OH's init sets
    /// SECBIT_NO_SETUID_FIXUP(+_LOCKED) and SECBIT_KEEP_CAPS(+_LOCKED)
    /// on every display service (composer_host, render_service,
    /// bootanimation) before exec — EINVAL there aborted the service
    /// with INIT_EKEEPCAP before it ever ran.
    pub securebits: u32,
    // Key-retention-service slots (ipc/keys.rs). Stored on the cred so
    // fork inheritance is the cred clone (reference process_keys.c);
    // exec clears the thread keyring (exec_keys()). Serial 0 is never
    // a valid key, but Option keeps "never set" explicit for the
    // session-lookup auto-install logic.
    pub thread_keyring: Option<u32>,
    pub process_keyring: Option<u32>,
    pub session_keyring: Option<u32>,
    /// KEYCTL_SET_REQKEY_KEYRING default (cred->jit_keyring in the
    /// reference): which keyring a future request_key() links into when
    /// the caller passes dest=0. KEY_REQKEY_DEFL_DEFAULT = 0.
    pub reqkey_defl: i32,
}

/// sc135 (Session Q): length of the per-task all-syscalls nr ring
/// (Task::last_calls). Shared by the dispatch-side writer and the
/// APP-PARK / thread-exit / APP-THR dump readers so the ring length
/// lives in exactly one place.
pub const LAST_CALLS_LEN: usize = 96;

/// sc138 (Session Q): length of the per-task FILTERED init ring
/// (Task::init_calls). The unfiltered last_calls ring drowns in the
/// parked child's {clock_gettime, writev, connect} retry loop — 96 raw
/// entries see only loop. init_calls SKIPS nr 113 (clock_gettime), 66
/// (writev), 203 (connect) always, and 98 (futex) when a0 is a
/// main-stack-top uaddr (the cond retry sites), so the ring retains the
/// init sequence proper: loader mmaps, prctls, mprotects, dlopen
/// openat/read/mmap cascades — the byte-diff substrate for working-vs-
/// parked children. init_a0 keeps the first argument of each entry
/// (prctl option / mmap addr / futex uaddr).
pub const INIT_CALLS_LEN: usize = 512;

/// sc143 (Session Q): one-shot latch of the task's FIRST 64 filtered
/// calls (the birth window: fork-exit -> hooks -> first dlopens) that
/// survives later loop-churn overwriting the deep ring.
pub const BIRTH_CALLS_LEN: usize = 64;

/// Task Control Block
///
///
/// Core field correspondence:
/// - state: task_struct::state
/// - pid: task_struct::pid
/// - tgid: task_struct::tgid (thread group ID)
/// - prio: task_struct::prio (dynamic priority)
/// - static_prio: task_struct::static_prio (static priority)
/// - normal_prio: task_struct::normal_prio
/// - policy: task_struct::policy
/// - context: cpu_context
/// - mm: task_struct::mm (memory descriptor)
/// - files: task_struct::files (file descriptor table)
/// - signal: task_struct::signal (signal handling)
///
/// Compatibility design:
/// - thread_info style fields at struct beginning
/// - tp register points to Task struct
/// - Kernel stack managed via kernel_sp field
#[repr(C)]
pub struct Task {
    /// Process flags (thread_info.flags)
    /// Bit definitions: TIF_SIGPENDING, TIF_NEED_RESCHED, etc.
    pub(super) ti_flags: AtomicU32,

    /// Preemption count (thread_info.preempt_count)
    /// > 0 means preemption disabled
    pub(super) ti_preempt_count: core::sync::atomic::AtomicI32,

    /// Kernel stack pointer (thread_info.kernel_sp)
    /// Points to top of kernel stack
    pub(super) ti_kernel_sp: core::sync::atomic::AtomicU64,

    /// User stack pointer (thread_info.user_sp)
    /// Saves user mode sp, used for trap return
    pub(super) ti_user_sp: core::sync::atomic::AtomicU64,

    /// Which CPU running on (thread_info.cpu)
    pub(super) ti_cpu: core::sync::atomic::AtomicI32,

    /// CPU affinity mask — bit i set = CPU i allowed
    pub(super) cpus_allowed: core::sync::atomic::AtomicU32,

    /// jiffies of this task's most recent task-context base-count DRAIN
    /// (the outermost guard's 1->0 release — a live lock pair completing).
    /// phantom-bits (2026-11-01): the tick-side PREEMPT-HEAL's cumulative
    /// observation bar false-fired on hot lock users (pb3: 887 heals/boot,
    /// 96% on 4 lock-hot pids, oage_jif=0-1 — a fresh 0->1 acquire inside
    /// the "2 s hold", i.e. the base drained between ticks and the 100 Hz
    /// sampler never caught the gaps). Every false heal strips a LIVE
    /// guard, marks need_resched, and preempts the holder MID-SECTION —
    /// stranding its lock for every other CPU (the both-CPU
    /// spin/deadlock amplifier). The heal now requires the base to have
    /// been nonzero CONTIGUOUSLY: any real 1->0 release stamps this field
    /// and re-arms the window. Direct zeroings (heal paths) do NOT stamp
    /// (they bypass sub_preempt_count_at), so genuine phantoms — whose
    /// +1 never drains — still heal after 2 s.
    pub(super) ti_pc_zero_jif: AtomicU64,

    /// CONSECUTIVE-PRESENT observation run for the SOFTIRQ field
    /// (stress-port r3, 2026-11-02): number of the task's own-CPU tick
    /// observations, in a row, that saw this field PRESENT since the
    /// last absence / legal drain / external zeroing. The tick-side
    /// PREEMPT-HEAL gates on run >= 200. This replaces the
    /// absence-stamp-age bar, which three producer families defeated:
    /// (1) stamp-age accrued for FREE while a task was off-CPU (the
    /// memory2/hunt2 strand: a task parked mid-section through a 100 s
    /// pick-skip strand resumed "instantly stale" and its first
    /// post-resume tick healed a LIVE guard — spinlock.rs underflow at
    /// the page-allocator zone lock); (2) jiffies is timebase-DERIVED
    /// (timer/x86_64.rs), so a >2 s host steal window inflates stamp
    /// age by 200+ ticks with zero ticks delivered — steal-window
    /// false-fires on live guards; (3) the pb3 release-side re-arm
    /// stored the SUPERSEDED ti_pc_zero_jif field (see below), so the
    /// per-field bar never actually re-armed on legal 1->0 drains and
    /// the 100 Hz sampler's missing of microsecond absent-gaps between
    /// guard pairs produced 200-present-sample streaks on lock-hot
    /// tasks (memory-hunt2: a heal storm, oage_jif=1 on every line —
    /// a fresh 0->1 acquire one jiffy before each "2 s hold" heal).
    /// Observation counting is immune to all three by construction:
    /// off-CPU and stolen CPUs deliver no ticks (run frozen), and the
    /// release-side reset (sub_preempt_count_at, on the field this
    /// gate reads) makes every legal drain re-arm regardless of
    /// sampler luck. The parked-with-nonzero-count shape stays owned
    /// by the parkwalk's 4 s PARKED-HEAL, which needs no tick.
    pub(super) ti_pc_sirq_present_run: AtomicU64,

    /// Same run counter for the HARDIRQ nesting field (bumped only when
    /// the extra-nesting predicate `>= 2 * HARDIRQ_OFFSET` holds at
    /// tick, so the observer's own in-flight level never counts). A
    /// phantom hardirq level with run >= 200 heals to exactly one kept
    /// level (ti_preempt_count_keep_tick_nesting — the still-pending
    /// irq_exit sub pairs against the kept level).
    pub(super) ti_pc_hirq_present_run: AtomicU64,

    /// Same run counter for the base PREEMPT field. Reset by: tick
    /// absent-sample, the legal outermost 1->0 release
    /// (sub_preempt_count_at), every external zeroing
    /// (ti_preempt_count_zero / clear_base / keep_tick_nesting), and
    /// the underflow clamp. Heals only at run >= 200.
    pub(super) ti_pc_pirq_present_run: AtomicU64,

    /// khungtaskd bookkeeping (review批次8): timestamp (ns) when this task
    /// was FIRST seen in D-state in the current window; 0 = not in D-state.
    /// Per-task storage replaces the old pid-indexed side table — real pids
    /// exceed MAX_TASKS, so the table silently skipped every real task.
    pub hung_task_since: core::sync::atomic::AtomicU64,

    /// Live wait-queue registrations naming this task (R127 wake-drop
    /// hunt): incremented by WaitQueueHead::add, decremented by every
    /// removal path (remove_count and the R127 stale eviction). A task
    /// exiting with this nonzero parked somewhere without unwinding —
    /// the exit-leak class the evdev poller showed on the OH hunts
    /// (thread gone, its poll entries still queued on /dev/input
    /// nodes, later stripped by a recycled-page remove). Maintained
    /// unconditionally (one atomic per park/unpark, against the
    /// irqsave lock pair those paths already pay); the diagnostic
    /// print at exit is hunt-gated.
    pub wq_regs: core::sync::atomic::AtomicU32,

    /// Timed-sleep shadow (ticktrap, r41v17): the deadline of the timed
    /// wait this task is currently parked in, if any. Set by the wait
    /// site at timer-arm time and cleared at scope exit (guard Drop);
    /// the 1 Hz heartbeat reads it WITHOUT touching user memory (the
    /// bwr_park_audit lesson: the heartbeat context cannot dereference
    /// another task's user pointers) and names the loss branch for any
    /// parked timed sleeper: deadline passed + no wheel entry = a lost
    /// fire/wake; deadline far future = an intended-or-garbage long
    /// sleep; entry present + not due = healthy.
    /// 0 = not in a timed wait.
    pub timed_sleep_deadline: core::sync::atomic::AtomicU64,

    /// 0 = none, 1 = hres (nanosleep class, deadline in time-CSR ticks),
    /// 2 = jiffy (futex/poll timeout class, deadline in jiffies).
    pub timed_sleep_kind: core::sync::atomic::AtomicU32,

    /// The wheel registration id of the armed wake timer (0 if arm
    /// failed — the wait site then never parks waker-less).
    pub timed_sleep_timer_id: core::sync::atomic::AtomicU64,

    /// Jiffies stamp of the arm (age gate for the heartbeat print).
    pub timed_sleep_armed_j: core::sync::atomic::AtomicU32,

    /// Requested duration (ms, capped) — the adjudicator between a
    /// garbage-huge userspace sleep and a short sleep whose wake was
    /// lost.
    pub timed_sleep_dur_ms: core::sync::atomic::AtomicU32,

    /// R12-5: lookup pin count — pid_hash_lookup increments, task_put
    /// decrements and frees at zero. Closes the lookup->use window that
    /// fed freed Task pointers into enqueue.
    pub task_refcnt: core::sync::atomic::AtomicU32,

    /// Per-task journal handle (R8-M3): the ext4 write path used a GLOBAL
    /// slot holding a pointer to a STACK-LOCAL handle — on SMP one CPU
    /// clobbered another's pointer and dereferenced a dead stack frame
    /// after the owner returned (Linux keeps this in task_struct).
    /// The current journal handle, TYPE-ERASED (rung-5): the jbd2
    /// Handle type is service-crate-owned; readers dereference as the
    /// fixed-offset capture header (INV-JSEAM-1) or hand the raw
    /// pointer to the crate's journal hook rows.
    pub journal_handle: core::cell::Cell<*mut ()>,

    /// NEW2 root-cause fix (R8-1b): true from pick time until __switch_to
    /// saves this task's context. A queued task with on_cpu set is not
    /// pickable by OTHER CPUs — the old code let another CPU pick a
    /// RUNNING task after __schedule unlocked the GRQ but before
    /// context_switch saved its sp/ra, resuming it from a STALE stack
    /// pointer (one task, two CPUs, one kernel stack — NEW2's double
    /// frees, len-0 BufferHeads, saved-ra corruption, fdtable teardown).
    /// The CPU switching the task out may still re-pick ITSELF (fast path).
    pub ti_on_cpu: core::sync::atomic::AtomicBool,

    /// RACE-FORENSICS (x86-smprace): CPU that currently owns the task's
    /// RESUMED continuation (-1 = none). Claimed at __switch_to's publish
    /// (right before the saved context is restored), released at the next
    /// publish. Unlike ti_on_cpu, nothing but the switch path touches it —
    /// a second claim while one is held is an unambiguous double-run.
    pub running_on_cpu: core::sync::atomic::AtomicI32,

    /// R56-heal pick-state machine (see PICK_PICKED/PICK_LIVE/PICK_REVOKED
    /// above). Arbitrates the slot-capture steal against the picking CPU's
    /// resume: the steal may only revoke a PICKED pick; a LIVE task is
    /// never touched. This is what the m25/m26/m27 gnome-boot pfault
    /// family (heal cpu=2 pid=311, Xorg) tripped over — a
    /// TCG-starved-but-live vCPU resumed its pick right after the heal
    /// re-enqueued it, running one task on two harts.
    pub ti_pick_state: core::sync::atomic::AtomicU32,

    /// R11 (oh-launch round 11, the terminal-release window): set ONLY
    /// when this task's FINAL context switch-out has really completed —
    /// written at the SWITCHING_OUT retire sites (sched.rs) in the
    /// INCOMING task's context, never on the abort path (current ==
    /// prev there). The terminal deschedule loop
    /// (schedule_dead_never_returns) must release the ownership pair
    /// BEFORE calling schedule() so the pick machinery can resolve
    /// another task (the sc191 v2 fix) — but from that release until the
    /// SWITCHING_OUT publish inside schedule(), the task reads
    /// {on_cpu=false, no marker} and the free-side waits alone would let
    /// the kernel stack be freed WHILE THE TASK IS STILL EXECUTING ITS
    /// OWN SCHEDULE CHAIN ON IT (every abort/retry cycle of the terminal
    /// loop re-opens the window — the rip=0x1/rip=heap/#GP-on-ret family
    /// at exit-storm time). This flag is the positive knowledge the free
    /// side requires: the real switch completed, the marker retired, the
    /// stack is referenced by nothing.
    pub terminal_switch_done: core::sync::atomic::AtomicBool,

    /// Positive knowledge that THIS task is still executing its
    /// terminal deschedule loop (boot-63 residue, 2026-11-21): set at
    /// schedule_dead_never_returns entry, cleared ONLY at the real
    /// switch-out retire sites (beside terminal_switch_done). The reap
    /// drainer's sustained arm requires it false — a pacing ZOMBIE/DEAD
    /// tail can hold {slotless, unmarked, !on_cpu} through abort-retry
    /// windows while its page must NOT be freed (the timing bet the
    /// 20-tick arm made before this flag existed).
    pub terminal_pacing: core::sync::atomic::AtomicBool,

    /// Scratch registers for trap handling (thread_info.a0/a1/a2)
    pub(super) ti_a0: core::sync::atomic::AtomicU64,

    pub(super) ti_a1: core::sync::atomic::AtomicU64,

    pub(super) ti_a2: core::sync::atomic::AtomicU64,

    /// Process state (volatile, visible across cores)
    pub(super) state: AtomicU32,

    /// Process ID
    pub(super) pid: Pid,

    /// Thread group ID (main process PID of thread)
    /// Single-threaded process: tgid == pid
    pub(super) tgid: Pid,

    /// Process group ID
    pub(super) pgid: u32,

    /// Process name (comm), 16 bytes NUL-terminated
    pub(super) comm: [u8; 16],

    /// Signal mask (blocked)
    ///
    /// Used for sigprocmask syscall
    pub sigmask: u64,

    /// Process credentials
    pub(super) cred: Cred,

    /// Scheduling policy
    pub(super) policy: SchedPolicy,

    /// Dynamic priority (0-139, higher value means lower priority)
    /// - 0-99: Real-time processes
    /// - 100-139: Normal processes
    pub(super) prio: i32,

    /// Static priority (120 is default for normal processes)
    pub(super) static_prio: i32,

    /// normal_prio: priority calculated from static_prio and scheduling policy
    pub(super) normal_prio: i32,

    /// Remaining time slice
    pub(super) time_slice: u32,

    /// RT scheduling priority (0-99, lower value = higher priority)
    ///
    /// Only valid when policy is SCHED_FIFO or SCHED_RR
    pub(super) rt_priority: u32,

    /// SCHED_RESET_ON_FORK request (set via sched_setscheduler/setattr):
    /// forked children are reset to SCHED_OTHER/nice 0 and the request is
    /// cleared in the child (Linux sched_reset_on_fork).
    pub(super) sched_reset_on_fork: core::sync::atomic::AtomicBool,

    /// CFS scheduling entity
    ///
    /// Contains vruntime, weight and other CFS scheduling info.
    /// pub(crate) so fair.rs can compute the entity-embedded timeline
    /// node's container-of offset (grq-10ns intrusive rbtree).
    pub(crate) sched_entity: crate::sched::entities::SchedEntity,

    /// RT scheduling entity
    ///
    /// Contains time slice and other RT-specific scheduling info
    pub(super) rt_entity: crate::sched::entities::SchedRtEntity,

    /// Deadline scheduling entity
    ///
    /// Contains deadline, runtime, period and other DL-specific info
    pub(super) dl_entity: crate::sched::entities::SchedDlEntity,

    /// RT run list node (for linking into RT priority queues)
    ///
    /// Used by the RT scheduler to link tasks at the same priority level
    pub rt_run_list: crate::list::ListHead,

    /// Architecture-specific thread state
    ///
    /// Stores FPU state, TLS pointer, etc.
    pub thread: crate::arch::thread::ThreadStruct,

    /// Address space (mm_struct)
    /// None for kernel threads, Some for user processes
    /// Use Arc for CLONE_VM sharing between threads
    pub(super) address_space: Option<alloc::sync::Arc<AddressSpace>>,

    /// Active address space (active_mm)
    ///
    /// For user processes: active_mm == mm
    /// For kernel threads: active_mm is borrowed address space (for accessing user memory)
    pub(super) active_mm: Option<*const AddressSpace>,

    /// Kernel stack
    /// TODO: Implement kernel stack allocation
    pub(super) kernel_stack: Option<*mut u8>,

    /// Kernel stack bottom (for overflow detection)
    pub(super) kernel_stack_bottom: usize,

    /// File descriptor table (files_struct)
    /// Use Arc for CLONE_FILES sharing between threads
    pub(super) fdtable: Option<alloc::sync::Arc<FdTable>>,

    /// Filesystem information (cwd, root, umask)
    ///
    /// Shared between threads when CLONE_FS is used
    pub(super) fs: Option<alloc::sync::Arc<crate::fs::FsStruct>>,

    /// Signal handling structure (signal_struct)
    /// Use Arc for CLONE_SIGHAND sharing between threads
    pub signal: Option<alloc::sync::Arc<SignalStruct>>,

    /// Pending signals (pending)
    pub pending: SigPending,

    /// PID namespace (None == initial namespace).
    pub ns_pid: Option<alloc::sync::Arc<crate::process::types::PidNamespace>>,

    /// PID hash table chain link (singly-linked for RCU-safe traversal).
    pub pid_hash_next: *mut Task,

    /// Fork child flag
    /// If true, this is a child process created by fork, needs to restore from ret_from_fork
    pub(super) is_fork_child: core::sync::atomic::AtomicBool,

    /// CPU-time accounting (kernel ticks, HZ per second). utime/stime are
    /// this task's own user/system time, sampled in the timer IRQ from the
    /// interrupted privilege mode (Linux account_process_tick style);
    /// cutime/cstime accumulate the same counters of reaped children at
    /// their exit. Read by /proc/[pid]/stat, times(2) and getrusage(2).
    pub utime_ticks: AtomicU64,

    pub stime_ticks: AtomicU64,

    /// Page faults taken by this task in user mode (Linux's minflt).
    /// Rux has no minor/major distinction — file-backed pages come
    /// from the page cache, not from a fault-time disk read — so every
    /// user fault is counted here and the major counter stays 0.
    pub min_flt: AtomicU64,

    /// Major page faults (Linux's majflt). Always 0 on Rux, see above.
    pub maj_flt: AtomicU64,

    /// Context-switch counters (Linux's task->nvcsw / task->nivcsw).
    /// A voluntary switch is one where the task went to sleep; an
    /// involuntary one is where the task was preempted while still
    /// runnable. Counted in the scheduler's context_switch and read by
    /// /proc/[pid]/status (voluntary_ctxt_switches /
    /// nonvoluntary_ctxt_switches).
    pub nvcsw: AtomicU64,

    pub nivcsw: AtomicU64,

    /// Per-task scheduler statistics (the /proc/[pid]/schedstat
    /// triple — T7 perf-support slice). runtime_ns accumulates on-cpu
    /// time at every switch-away, wait_ns the runqueue wait at every
    /// pick, timeslices counts real picks (a same-task repick after a
    /// tick is NOT a new timeslice — Linux counts switches-to). The
    /// two stamps (wait_start, exec_start) are written only under the
    /// GRQ lock; the three counters are readable lock-free.
    pub schedstat_runtime_ns: AtomicU64,

    pub schedstat_wait_ns: AtomicU64,

    pub schedstat_timeslices: AtomicU64,

    pub schedstat_wait_start: AtomicU64,

    pub schedstat_exec_start: AtomicU64,

    /// F1 service-block bridge (INV-F1, see process/task_ext.rs):
    /// allocated at task creation, freed at slot release — the slot
    /// lifecycle owns both blocks. Followed only by cold paths;
    /// hot fields live above, in the first cache lines.
    pub ext: TaskExtHandle,

    /// Thread group leader (task_struct::group_leader).
    ///
    /// Self-pointer for a leader / single-threaded process; points at the
    /// leader for CLONE_THREAD members. The ring `next_thread` below links
    /// every live member (leader included) into a Linux-style circular
    /// thread_group list. NULL only between construction and first fixup —
    /// `group_leader_ptr()` heals it to self.
    pub(super) group_leader: *mut Task,

    /// Next member of the thread group ring (task_struct::thread_group).
    ///
    /// The ring is: leader -> m_n -> ... -> m_1 -> leader (head insert in
    /// thread_group_join). Members unlink themselves in exit. Guarded by
    /// PROCESS_TREE_LOCK (Linux uses ->siglock / threadgroup_lock).
    pub(super) next_thread: *mut Task,

    /// Live thread count in this group (task_struct::signal->nr_threads).
    ///
    /// AUTHORITATIVE ON THE LEADER ONLY. Initialized to 1 at construction;
    /// thread_group_join/leave maintain it on the leader.
    pub(super) nr_threads: AtomicU32,

    /// Exited group members awaiting deferred resource release
    /// (Linux delay_put_task_struct analogue). Leader-only. A non-leader
    /// thread cannot free its own kernel stack/Task slot while executing on
    /// them and no one wait4()s threads, so the exiting member parks itself
    /// here; the leader sweeps it once on_cpu clears (another member exit,
    /// exec, or the leader's release_task).
    pub dead_threads: Spinlock<alloc::vec::Vec<*mut Task>>,

    /// Children list
    ///
    /// This is a list head, all children are linked here via their sibling field
    pub children: ListHead,

    /// Sibling process list node
    ///
    /// Used to link this process to parent's children list
    pub sibling: ListHead,

    /// Parent's children list head pointer (for next_sibling boundary check)
    ///
    /// When process is added to parent, saves address of parent's children
    /// Used by next_sibling() to detect list end
    pub(super) parent_children_head: *mut ListHead,

    /// Parent process
    pub(super) parent: Option<*const Task>,

    /// mm-slot lock — serializes the `address_space` Option<Arc> swap in
    /// set_address_space (exec / exit_mm / fork install) against foreign
    /// readers cloning the Arc (address_space_arc: ptrace, OOM scan, rmap,
    /// futex identity, taskdump). Without it a reader could load the Arc
    /// pointer, the owner's exit drop the last reference (freeing the
    /// MmStruct), and the reader's clone then resurrect freed memory with
    /// a garbage refcount — the resurrected Arc's later drop runs
    /// MmStruct::drop a SECOND time (the FUT-STALE / PTLEDGER
    /// DOUBLE-FREE root-sharing family, fs-writestorm KVM round: pid 555
    /// freed root 0x62961, pid 554 re-walked it and #GP'd in the VMA
    /// btree). irqsave: trap/panic-context readers exist (riscv64 trap
    /// printer). Leaf lock: never taken while holding other locks, and
    /// the old Arc is dropped AFTER the unlock (its Drop may run the
    /// whole page-table teardown).
    pub(super) mm_slot_lock: Spinlock<()>,

    /// Exit code (valid in Zombie state)
    pub(super) exit_code: i32,

    /// Signal delivered to the parent at this task's exit (clone/
    /// clone3 `exit_signal`; Linux stores it on the task and uses it
    /// in do_notify_parent — SIGCHLD unless the clone asked otherwise,
    /// LTP clone301 expects a clone3(SIGUSR2) child to SIGUSR2 its
    /// parent). 0 = no signal.
    pub(super) exit_signal: u8,

    /// Signal that stopped the process (valid in Stopped state)
    pub(super) stop_signal: i32,

    /// Whether WUNTRACED stop has already been reported to parent
    pub stop_reported: core::sync::atomic::AtomicBool,

    /// Deferred-release marker (69th, the 105-family root cause): set
    /// on a reaped GROUP LEADER whose release_task could not drain its
    /// dead_threads list within the bounded wait — a mid-exit member
    /// stuck in the fdreap grace / a scheduler strand would otherwise
    /// hot-spin the REAPER (init) inside wait4 forever, killing the
    /// param service (every later ohos.ctl.start returns 105). The
    /// reaper completes the reap SEMANTICS and returns; the storage
    /// free is finished by the LAST dying member's
    /// sweep_dead_threads. False on every normal task.
    pub release_deferred: core::sync::atomic::AtomicBool,

    /// Wait queue for child process exit events
    ///
    /// Parent processes wait on this queue when calling wait4/waitpid
    /// Child process exit wakes up parent via this queue
    pub wait_chldexit: crate::process::types::WaitQueueHead,

    /// vfork parent task pointer
    ///
    /// When CLONE_VFORK is used, the child stores the parent's task pointer here.
    /// The child wakes the parent when it calls execve() or _exit().
    /// 0 = not a vfork child (or vfork already completed).
    pub(super) vfork_parent: core::sync::atomic::AtomicU64,

    /// Fork child's PtRegs pointer
    /// When is_fork_child is true, this pointer points to child's PtRegs
    /// Scheduler will use this PtRegs to restore child state
    pub(super) fork_pt_regs: core::sync::atomic::AtomicU64,

    /// OpenHarmony access token id (per-process, /dev/access_token_id).
    /// Inherited by fork children.
    pub access_token: AtomicU64,

    /// OpenHarmony foreground token id. NOT inherited (fork clears it).
    pub access_ftoken: AtomicU64,

    /// Parent-death signal (0 = disabled)
    pub pdeath_signal: u32,

    /// SUID dumpable flag (0=SUID_DUMP_DISABLE, 1=SUID_DUMP_USER)
    pub dumpable: u32,

    /// Set at the first successful execve: once a process has exec'd, its
    /// parent may no longer move it between process groups (Linux
    /// sys_setpgid EACCES; LTP setpgid03).
    pub(super) execed: core::sync::atomic::AtomicBool,

    /// Session ID
    pub(super) sid: u32,

    /// Process execution domain (personality(2)).
    ///
    /// Store-and-readback semantics: personality(x) stores x and returns
    /// the previous value; personality(0xffffffff) is the query sentinel
    /// (returns the current value unchanged). Inherited across fork,
    /// kept across execve (exec domains like setarch --addr-no-randomize
    /// rely on the flag surviving exec). 0 = PER_LINUX.
    pub personality: u32,

    /// Signal stack (sigaltstack)
    pub sigstack: crate::signal::SignalStack,

    /// Signal frame address (in user space)
    pub sigframe_addr: u64,

    /// Signal frame (kernel space backup)
    pub sigframe: Option<crate::signal::SignalFrame>,

    /// Process heap boundary (brk)
    ///
    /// Points to end address of process heap, managed by sys_brk
    /// Initial value is 0, set to default on first brk call
    pub(super) brk: core::sync::atomic::AtomicU64,

    /// OOM score adjustment [-1000, 1000]
    ///
    /// -1000 = immune from OOM kill (OOM_SCORE_ADJ_MIN)
    /// 0 = default
    /// 1000 = maximum priority to be killed (OOM_SCORE_ADJ_MAX)
    /// Follows Linux's signal->oom_score_adj
    pub(super) oom_score_adj: core::sync::atomic::AtomicI32,

    /// I/O scheduling priority (ioprio_set/ioprio_get, block/ioprio ABI):
    /// (class << 13) | level; 0 = IOPRIO_CLASS_NONE level 0. Stored per
    /// task and inherited across fork like the reference kernel's
    /// task_struct::ioprio (not yet fed into I/O scheduling decisions).
    pub(super) ioprio: core::sync::atomic::AtomicI32,

    /// Executable file path
    ///
    /// Stores process's executable file path
    /// Used for /proc/self/exe etc.
    pub(super) exe_path: alloc::boxed::Box<[u8]>,

    /// Per-process semaphore undo table.
    /// Records SEM_UNDO adjustments to be reversed on process exit.
    // pub (visibility tax of the ipc flip): the sysv_sem policy's
    // exit cleanup reads/takes this table through the IpcHooks row.
    pub sem_undo: Spinlock<alloc::vec::Vec<crate::ipc::SemUndoEntry>>,

    /// Interval timers (ITIMER_REAL=0, ITIMER_VIRTUAL=1, ITIMER_PROF=2).
    /// Each stores the kernel timer ID (0 = disarmed).
    pub itimer_ids: [core::sync::atomic::AtomicU64; 3],

    /// ITIMER_VIRTUAL (SIGVTALRM) CPU-time state: (deadline_ns,
    /// interval_ns) against the sched entity's sum_exec_runtime.
    /// deadline==0 disarms. Atomics only — touched from scheduler_tick
    /// (IRQ context). POSIX: not inherited across fork, cleared on exec.
    pub itimer_virt: [AtomicU64; 2],

    /// ITIMER_PROF (SIGPROF) CPU-time state: (deadline_ns, interval_ns).
    pub itimer_prof: [AtomicU64; 2],

    /// ITIMER_REAL (SIGALRM) wall-clock state: (deadline_ns against the
    /// monotonic clock, interval_ns). deadline==0 disarms. Mirrored by
    /// the timer-wheel entry in itimer_ids[0]; this copy exists so
    /// getitimer/setitimer can report the REMAINING time (alarm(2)
    /// return value via musl).
    pub itimer_real: [AtomicU64; 2],

    /// POSIX timers created via timer_create.
    /// Stores (timer_id, clock_id, interval_jiffies, sigev_signo, sigev_notify, overrun_count).
    pub posix_timers: Spinlock<alloc::vec::Vec<PosixTimerState>>,

    /// Resource limits (RLIMIT_*): [(rlim_cur, rlim_max); 16].
    /// Initialized to kernel defaults; getrlimit/setrlimit/prlimit64 read
    /// and write here, and the enforcement points (alloc_fd, mmap/brk,
    /// fork NPROC, scheduler_tick CPU) consult it.
    pub rlimits: Spinlock<[(u64, u64); RLIM_NLIMITS]>,

    /// Process CPU-time second at which SIGXCPU was last delivered
    /// (RLIMIT_CPU soft limit: Linux re-sends SIGXCPU every further
    /// second of CPU time; 0 = never sent). Atomics only — read from
    /// scheduler_tick (IRQ context).
    pub(super) cpu_time_last_sigxcpu: AtomicU64,

    /// Mount namespace (None == initial namespace).
    pub ns_mount: Option<alloc::sync::Arc<crate::process::types::MountNamespace>>,

    /// UTS namespace (None == initial namespace).
    pub ns_uts: Option<alloc::sync::Arc<crate::process::types::UtsNamespace>>,

    /// IPC namespace (None == initial namespace; stub).
    pub ns_ipc: Option<alloc::sync::Arc<crate::process::types::IpcNamespace>>,

    /// PID namespace that future children are created in (unshare/setns
    /// CLONE_NEWPID target; None == same as ns_pid).
    pub ns_pid_for_children: Option<alloc::sync::Arc<crate::process::types::PidNamespace>>,

    /// Network namespace (None == initial namespace; stub).
    pub ns_net: Option<alloc::sync::Arc<crate::process::types::NetNamespace>>,

    /// User namespace (None == initial namespace; stub).
    pub ns_user: Option<alloc::sync::Arc<crate::process::types::UserNamespace>>,

    /// PID of this task inside its pid namespace (== pid for the initial
    /// namespace; 1 for a CLONE_NEWPID namespace init).
    pub ns_local_pid: u32,

    /// Per-level pid numbers for every pid namespace STRICTLY between
    /// this task's own ns and the init ns — the reference struct pid's
    /// upid[] chain minus its two ends (own-ns pid is `ns_local_pid`;
    /// the init-ns pid is the global `pid`). A depth-1 task (own ns's
    /// parent is init) has an EMPTY chain; a depth-2 task carries the
    /// number the intermediate ns assigned it. Built once in
    /// copy_namespaces; immutable afterwards.
    pub ns_upids: alloc::vec::Vec<(
        alloc::sync::Arc<crate::process::types::PidNamespace>,
        u32,
    )>,

    /// Seccomp mode: 0 = SECCOMP_MODE_DISABLED, 1 = STRICT, 2 = FILTER.
    pub seccomp_mode: AtomicU32,

    /// Loaded classic-BPF filter program (single program; loading a new one
    /// replaces it — Linux stacks filters, recorded divergence).
    pub seccomp_filter: Spinlock<Option<alloc::vec::Vec<SockFilter>>>,

    /// no_new_privs attribute (PR_SET_NO_NEW_PRIVS, Linux task_struct::
    /// no_new_privs): when set, execve ignores setuid/setgid bits (and
    /// file capabilities) — the exec'd program runs with the same
    /// credentials — and seccomp filter attach is legal without
    /// CAP_SYS_ADMIN. ONE-WAY at the prctl ABI (arg2 must be exactly 1;
    /// no clear exists); this setter is the kernel-internal API used by
    /// prctl, fork inheritance, and tests. Inherited across fork and
    /// exec (Linux semantics).
    pub no_new_privs: AtomicBool,

    /// cgroup membership (Linux task_struct::cgroups analogue). NULL =
    /// implicit root — kernel threads and early-boot tasks carry no
    /// limits (Linux kthreads bypass cgroup limits too). The pointee is
    /// a `CgroupNode` inside an Arc allocation that is NEVER freed
    /// (rmdir only unlinks it), so the raw pointer needs no refcount.
    pub(super) cgroup_ptr: *mut crate::sched::cgroup::CgroupNode,

    /// cgroup v2 memory controller: bytes this task has charged to its
    /// cgroup chain via mmap(2). Charges and uncharges (munmap / exit /
    /// cgroup.procs migration) all flow through this counter so the
    /// cgroup's memory.current can never over-subtract: execve's
    /// internal ELF mappings and brk never charge, so they never
    /// uncharge either (documented RSS-approximation simplification).
    pub(super) cgroup_mem_charged: AtomicU64,

    /// 63rd session (strand-wall hunt): persistent per-task pick/wake
    /// evidence, kept ON the task so no address-keyed side table can
    /// steal it mid-strand (the 61st's 256-slot SKIP_HIST collided).
    /// All writes are gated behind sched::sched_hist_armed()
    /// (rux:fuxstuck (retired debug=fuxstuck)|schedtrace) and happen under the GRQ lock; the
    /// census reads them lock-free. Ring entries pack
    /// kind:4|why:8|jiffies:30|aux:22 (kind: 1=link 2=refuse 3=pick
    /// 4=heal-claim 5=heal-oncpu; see sched::sched_hist_ring).
    ///
    /// The decisive pair is pick_n vs the exec clock: a strand victim
    /// with pick_n == 0 and pick_visit_n == 0 was NEVER VISITED by a
    /// pick (wall/stash class), while a growing pick_n over a frozen
    /// exec clock is the picked-but-never-executes class (the R56d
    /// lying-slot shape: next == prev fast-path repicks that never
    /// restore the context).
    pub pick_ring: [AtomicU64; 16],

    /// Total events ever appended; the slot is (seq & 15).
    pub pick_ring_seq: AtomicU64,

    /// Pick-walk refusals of this task (a visit that ended in a skip).
    pub pick_visit_n: AtomicU64,

    /// Refusals per R76 reason code, indices 0..4 == why 1..5
    /// (1 on_cpu, 2 foreign live claim, 3 affinity, 4 throttled,
    /// 5 stale healed).
    pub pick_refuse_why: [AtomicU64; 5],

    /// Jiffies of the first refusal in the current strand episode
    /// (0 = never refused since boot).
    pub pick_first_refuse_jif: AtomicU64,

    /// Times this task was picked (mark_picked_on_cpu), repicks
    /// included.
    pub pick_n: AtomicU64,

    /// Jiffies of the last pick (0 = never picked).
    pub pick_last_jif: AtomicU64,

    /// Successful run-queue links (wake/fork/heal re-insert).
    pub wake_n: AtomicU64,

    /// Jiffies of the last link (0 = never linked since boot).
    pub wake_last_jif: AtomicU64,

    /// sched_clock() ns at the last link — the wait window opened at
    /// enqueue and closed at pick (0 = closed/never opened).
    pub wait_open_ns: AtomicU64,

    /// Last link→pick wait, ns (diagnostic read).
    pub wait_last_ns: AtomicU64,

    /// Cumulative link→pick wait, ns (deltas between census bursts
    /// give the per-phase wake-to-run cost the FFRT group pays).
    pub wait_sum_ns: AtomicU64,

    pub wait_max_ns: AtomicU64,
}

/// One classic BPF instruction (struct sock_filter): { code, jt, jf, k }.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SockFilter {
    pub code: u16,
    pub jt: u8,
    pub jf: u8,
    pub k: u32,
}

/// Number of RLIMIT_* resources (matches Linux RLIM_NLIMITS).
pub const RLIM_NLIMITS: usize = 16;

/// Per-process POSIX timer state.
pub struct PosixTimerState {
    /// Kernel timer ID (0 = disarmed).
    pub kernel_timer_id: u64,
    /// Clock ID (CLOCK_REALTIME=0, CLOCK_MONOTONIC=1, and the CPU-time
    /// clocks 2/3 whose expiry is checked in scheduler_tick).
    pub clock_id: i32,
    /// Interval in jiffies (0 = one-shot).
    pub interval_jiffies: u64,
    /// Signal number to deliver.
    pub sigev_signo: i32,
    /// sigev_notify (SIGEV_SIGNAL=0, SIGEV_NONE=1).
    pub sigev_notify: i32,
    /// Overrun count.
    pub overrun_count: i32,
    /// User-visible timer ID.
    pub user_timer_id: i32,
    /// CPU-time deadline (ns on sum_exec_runtime; 0 = none armed) for
    /// CLOCK_PROCESS/THREAD_CPUTIME_ID timers. Atomics: scheduler_tick
    /// reads/re-arms them from timer-IRQ context where taking the
    /// posix_timers spinlock could deadlock against a syscall holder.
    pub cputime_deadline_ns: core::sync::atomic::AtomicU64,
    /// CPU-time re-arm interval (ns; 0 = one-shot).
    pub cputime_interval_ns: core::sync::atomic::AtomicU64,
    /// Wall-clock absolute expiry (time CSR ticks; 0 = none) — precise
    /// it_value readback for wheel-armed timers.
    pub wall_deadline_ticks: core::sync::atomic::AtomicU64,
    /// Programmed interval in ns (precise it_interval readback).
    pub interval_ns: u64,
}

// ---------------------------------------------------------------------------
// the task service block behind Task's bridge field (INV-F1) and its
// typed handle
// ---------------------------------------------------------------------------

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

    // ---- family (c): robust_list / futex / sigmask-restore state ----
    // All framework touchpoints are cold: exit runs them through the
    // ProcessHooks rows (exit_robust_list / exit_clear_child_tid),
    // the futex residency flag is set/cleared at futex-wait syscall
    // boundaries and read by exit cleanup plus the heartbeat
    // histogram, and the sigmask-restore pair is written by the
    // sigsuspend/ppoll-class syscalls and consumed inside do_signal's
    // delivery loop (signal frequency, not per-context-switch).

    /// set_tid_address() pointer: futex-woken at exit (CLONE_CHILD_CLEARTID).
    pub clear_child_tid: *mut i32,

    /// set_robust_list() head/len: walked by exit_teardown only.
    pub robust_list_head: *const u8,
    pub robust_list_len: usize,

    /// Residency flag: this task is linked into a futex wait bucket
    /// (futex_cleanup gates its 512-bucket scan on it).
    pub futex_wait_enqueued: AtomicBool,

    /// restore_saved_sigmask pair (rt_sigsuspend / pselect-class):
    /// restored on every trip back to userspace from do_signal.
    pub sigmask_restore: u64,
    pub sigmask_restore_valid: bool,
}

/// The typed bridge handle (the journal_handle precedent, minus the
/// type erasure: the pointee is framework-typed until the namespace
/// family flips this block into the service crate). INV-F1 (see the
/// module doc) governs its lifetime: allocated at task creation,
/// freed at slot release, owned by the slot lifecycle.
pub struct TaskExtHandle(pub(super) alloc::boxed::Box<TaskExt>);

// ---------------------------------------------------------------------------
// the typed task handles: the borrowed one and the pinned one
// ---------------------------------------------------------------------------

/// Unpinned borrowed handle: syscall-context short reads.
///
/// Contract (unchanged from `pid_hash_lookup`'s doc): the pointer stays
/// valid for a short read because the caller executes in syscall
/// context on this CPU and the reaper cannot run here mid-read. Do NOT
/// hold across `schedule()`, sleeps, or any path that yields the CPU;
/// do NOT store; use [`TaskRef`] when the handle must outlive such a
/// point.
pub struct TaskBorrow(pub(super) NonNull<Task>);

/// Pinned task handle owning a `task_refcnt` pin (R12-5).
///
/// `lookup` takes the pin; `Drop` releases it with `task_put` (the
/// last put frees the task slot). Valid across scheduling points and
/// cross-CPU wake/notify paths.
///
/// INV-TASKREF-1 (pin exclusivity, added by the slim4-fix lane
/// 2026-11-20): the handle owns its pin EXCLUSIVELY. Callers must NOT
/// call `task_put` on `as_ptr()` while the handle is alive — Drop
/// releases the same pin, so a manual put is a double release: the
/// receiver's `task_refcnt` erodes by one per call and `free_task_slot`
/// eventually fires on a task still linked in the pid hash, its
/// parent's child list, and the timer/runqueue structures (the slim4
/// fio-corruption regression: signal receivers freed early, wait4
/// walking a pid=0xdeadbeef page). A raw consumer that must release
/// the pin itself takes ownership with [`TaskRef::into_raw`] (which
/// `forget`s the handle and its Drop).
pub struct TaskRef(pub(super) NonNull<Task>);

// ---------------------------------------------------------------------------
// the wait queue: head, entry and wake hint
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum WakeUpHint {
    /// Normal wake up
    Normal = 0,
    /// Async wake up (don't actually wake process, just mark)
    Async = 1,
    /// Synchronous wake (reference WF_SYNC): the waker and wakee are a
    /// handoff pair — the waker is about to block. The scheduler stacks
    /// the wakee with the waker (never dispatches it onto another busy
    /// CPU to run in parallel). Used by binder's sync-txn/reply wakes
    /// (wake_up_interruptible_sync in binder.c, 66th session parity).
    Sync = 2,
}

#[repr(C)]
pub struct WaitQueueEntry {
    /// Associated task
    pub(super) task: *mut Task,
    /// Exclusive flag (WQ_FLAG_EXCLUSIVE)
    pub(super) exclusive: bool,
    /// Whether woken
    pub(super) woken: AtomicBool,
    /// The task's pid at registration time (wake-drop fix, R127):
    /// entries are matched by (task pointer, pid) so a Task page that
    /// was freed and RECYCLED by a new task cannot be mistaken for the
    /// old owner. Pids are unique per boot (monotonic until wraparound
    /// far beyond any session's spawn volume), so the pair is a sound
    /// incarnation discriminator; free_task_slot's TASK_POISON write
    /// also fails the compare, catching dead-but-not-reused pages.
    pub(super) pid: u32,
}

#[repr(C)]
pub struct WaitQueueHead {
    /// Wait queue list
    /// Uses Vec to store waiting processes
    pub(super) list: Spinlock<Vec<WaitQueueEntry>>,
    /// Entries currently on the list (leg-36 wake fast path).
    ///
    /// Maintained ONLY inside the list lock's critical sections
    /// (insertions increment, retains decrement by what they removed),
    /// so `0` is a lock-free proof that the list is empty — the same
    /// contract as Linux's `wq_has_sleeper()` reading `head->next`
    /// before taking the wait-queue lock. `wake_up` loads it with
    /// Acquire FIRST and returns without touching the irqsave lock
    /// when it reads 0: on a box where the consumer polls instead of
    /// sleeping (hilogd's drain loops, pbench socket pairs), that
    /// empty-wake lock was paid on every single event.
    ///
    /// Lost-wake safety (same argument as Linux's): a waker that
    /// reads 0 runs strictly before the sleeper's in-lock increment
    /// becomes visible; the sleeper's condition re-check AFTER
    /// prepare_to_wait (every wait site in this kernel re-checks —
    /// see wait_event and unix_wait_round) then observes the waker's
    /// condition mutation, which precedes its wake call. Either the
    /// waker sees the waiter, or the waiter sees the event.
    pub(super) waiters: AtomicUsize,
}

// ---------------------------------------------------------------------------
// one slot of the exec stack-zero ring
// ---------------------------------------------------------------------------

pub struct ZeroEntry {
    pub pid: core::sync::atomic::AtomicU32,
    pub base: core::sync::atomic::AtomicU64,
    pub len: core::sync::atomic::AtomicU64,
}

// ---------------------------------------------------------------------------
// the namespaces: the six kinds, the mount-table row, the namespace
// handle, the UTS buffer length and the nsfs file content
// ---------------------------------------------------------------------------

/// Linux __NEW_UTS_LEN = 64 (65 with NUL; we store without NUL).
pub const UTS_LEN: usize = 64;

pub struct UtsNamespace {
    /// nsfs inode number (unique per namespace instance).
    pub inum: u64,
    /// NUL-terminated hostname buffer.
    pub hostname: Spinlock<[u8; UTS_LEN]>,
    /// NUL-terminated domainname buffer.
    pub domainname: Spinlock<[u8; UTS_LEN]>,
}

/// One row of the mount table (mirrors the legacy global registry tuple).
#[derive(Clone)]
pub struct MountEntry {
    /// Permanent mount ID (Linux struct mount.mnt_id, allocated once per
    /// mount from a global counter). /proc/self/mountinfo emits THIS id —
    /// the old generator numbered rows POSITIONALLY over ns_get_mounts(),
    /// and ns_register_mount's retain+push moved re-registered rows to
    /// the Vec tail, so the same id re-paired with a different mount
    /// between two reads. systemd's mount monitor diffed those phantom
    /// changes against its table and failed freshly-mounted units
    /// ("Mount process finished, but there is no mount" — the
    /// sys-kernel-tracing class). Ids are stable for the row's life; a
    /// re-registration of the SAME mountpoint (flags refresh, row
    /// relocation) keeps the row's id, like remount keeps mnt_id.
    pub mount_id: u32,
    pub device: String,
    pub mount_point: String,
    pub fs_type: String,
    pub flags: String,
}

pub struct MountNamespace {
    /// nsfs inode number.
    pub inum: u64,
    /// Namespace-private root dentry. Captured at ns creation from the
    /// global VFS root (CLONE_NEWNS does not move "/" — it only private-
    /// copies the mount table). Reserved as the pivot_root bookkeeping
    /// anchor; path_lookup still walks the shared dentry tree.
    pub root: Spinlock<Option<Arc<crate::fs::dentry::Dentry>>>,
    /// Private copy of the mount table. Copy-on-write: modifications after
    /// CLONE_NEWNS only touch this Vec, never the parent's.
    pub mounts: Spinlock<Vec<MountEntry>>,
    /// sc98: the actual VfsMountInternal objects this namespace attached
    /// into the SHARED dentry tree after creation. Rux attaches every
    /// mount globally (there is one dentry tree), so without a per-ns
    /// list a dying private namespace would leave its mounts — and their
    /// VFS_STATE.mounts registry entries — attached forever. The OH
    /// appspawn child CLONE_NEWNS'es and makes ~50 sandbox binds; killed
    /// children (the launcher's LIFECYCLE_TIMEOUT restart loop) leaked
    /// them all, the registry grew into the thousands, and every later
    /// MS_REC bind cloned by scanning the whole registry (10-25 ms per
    /// mount) until NO spawn could finish inside appspawn's 3 s child
    /// watchdog. Linux unmounts a private namespace's mounts when its
    /// last task exits; Drop below restores exactly that contract.
    pub vfs_mounts: Spinlock<Vec<Arc<crate::fs::dentry::VfsMountInternal>>>,
}

pub struct PidNamespace {
    /// nsfs inode number.
    pub inum: u64,
    /// Parent (enclosing) namespace; None for the init namespace.
    #[allow(dead_code)] // structural (level already derived at creation)
    pub parent: Option<Arc<PidNamespace>>,
    /// Depth below the init namespace (init == 0).
    pub level: u32,
    /// Last ns-local pid handed out.
    pub last_pid: AtomicU32,
}

/// Net namespace stub. Network devices and addresses are GLOBAL (shared by
/// every instance); creating one only yields a distinct /proc/[pid]/ns/net
/// identity for setns/open purposes.
pub struct NetNamespace {
    pub inum: u64,
}

/// IPC namespace stub. SysV IPC objects are global.
pub struct IpcNamespace {
    pub inum: u64,
}

/// User namespace stub. uid/gid mapping is identity (kernel uid == ns uid);
/// only the initial namespace is supported. A new instance exists purely so
/// /proc shows a distinct ns and CLONE_NEWUSER does not fail.
pub struct UserNamespace {
    pub inum: u64,
    #[allow(dead_code)] // structural (identity mapping stub)
    pub parent: Option<Arc<UserNamespace>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NsKind {
    Uts,
    Mnt,
    Pid,
    Net,
    Ipc,
    User,
}

/// Namespace handle kept alive for open ns fds.
#[derive(Clone)]
pub enum NsRef {
    Uts(Arc<UtsNamespace>),
    Mnt(Arc<MountNamespace>),
    Pid(Arc<PidNamespace>),
    Net(Arc<NetNamespace>),
    Ipc(Arc<IpcNamespace>),
    User(Arc<UserNamespace>),
}

pub(super) struct NsFileContent {
    pub(super) data: Vec<u8>,
}
