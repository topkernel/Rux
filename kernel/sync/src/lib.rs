//! The SYNC SERVICE CRATE (sync-upper lane): the sync subsystem's cold
//! majority under a crate-level unsafe-deny boundary — the futex
//! diagnostic/forensic family (src/futex_diag.rs) and the condvar
//! (zero framework users, zero unsafe).
//!
//! What stays framework-side (kernel/core/src/sync/ — the lane's stay
//! verdicts, each recorded in the lane report):
//! - the futex MECHANISM (wait/wake/requeue/do_futex/sys_futex_handler,
//!   the waiter pool, the hash buckets) and futex_stuck_census (the
//!   timer tick calls it by name);
//! - the fuxwatch watchpoint machinery the x86 trap fault/step paths
//!   call by name;
//! - every lock primitive interior: spinlock, rwlock, seqlock, percpu,
//!   lockkey, edges (hot files), mutex (SleepMutex — hot trap/sched/vfs
//!   users), rwsem (MmStruct fields + the primitive state machine),
//!   semaphore (File.io_lock field + the fdreap static), rcu (the
//!   grace-period machine, INV-RCU-1 rows), futex_pi (called by name
//!   from the staying do_futex; the PI protocol on the wake path).
//!
//! The framework reaches THIS crate through the SyncDiagHooks table
//! (kernel/core/src/api/sync.rs — 16 cold rows; the do_futex /
//! census latch sites are arm-gated, the S1 pattern). Sibling service
//! crates (ipc binder_diag, process exit_teardown) reach the
//! moved functions DIRECTLY at their historical rux::sync::hotpath::X
//! paths through the union module below (the allowed downward service
//! edge — the binder_diag precedent, syscall -> ipc).

// FORBID since the flip: the moved futex diagnostic family and condvar
// carried zero unsafe blocks of their own (futex.rs's only unsafe is
// the Waiter Send/Sync pair, which stayed with the waiter pool); the
// raw task/page-table reads ride the framework's audited
// rux::sync::unsafe walkers.
#![forbid(unsafe_code)]
#![no_std]
#![feature(alloc_error_handler, linkage)]
#![recursion_limit = "4096"]

// Root re-exports: the framework modules appear at THIS crate's root so
// the service files' crate::X paths resolve unchanged (the net recipe).
pub use rux::{arch, cmdline, config, console, debug, drivers, errno, fs, interrupt,
    ipc, mm, net, process, safety, sched, security, signal, syscall, timer, trace};
pub use rux::{pr_info, pr_warn, pr_err, pr_debug, println};
extern crate alloc;

// The service files (THIS crate's own — the sole copies), flat at the
// crate root (seed-core-design 2c.2).
pub mod futex_diag;
pub mod condvar;

// The framework's whole sync module at the historical path (the union
// the sibling services consumed — the merged view `pub use sync;` in the
// ipc/process/syscall crates resolves through it) — the explicit `pub mod
// futex` below shadows the glob's futex module and re-exports BOTH
// halves: the staying mechanism from the framework and the moved
// diagnostic family from this crate's futex_diag.
pub use rux::sync::{r#unsafe, hotpath::*, types::*};

pub mod futex {
    // The names the framework's futex module exported before it became a
    // section of rux::sync::hotpath (seed-core pass 2), spelled out: a glob
    // over hotpath would re-export every section's public items.
    pub use rux::sync::types::{FuxWakeOp, FutexKey, Waiter, FutexChain};
    pub use rux::sync::hotpath::{
        FUTEX_WAIT,
        FUTEX_WAKE,
        FUTEX_FD,
        FUTEX_REQUEUE,
        FUTEX_CMP_REQUEUE,
        FUTEX_WAKE_OP,
        FUTEX_LOCK_PI,
        FUTEX_UNLOCK_PI,
        FUTEX_TRYLOCK_PI,
        FUTEX_WAIT_BITSET,
        FUTEX_WAKE_BITSET,
        FUTEX_WAIT_REQUEUE_PI,
        FUTEX_CMP_REQUEUE_PI,
        FUTEX_LOCK_PI2,
        FUTEX_PRIVATE_FLAG,
        FUTEX_CLOCK_REALTIME,
        FUTEX_CMD_MASK,
        FXDV_PID,
        FXRS_TGID,
        FXRS_WORDS,
        fxrs_word_add,
        fxrs_word_watched,
        FUXW_RING_SZ,
        FUXW_RINGS,
        fuxw_push,
        FUX_POOL_USED,
        FUX_POOL_FULL_J,
        RWFRZ_WATCH,
        RWFRZ_WATCH_BUDGET,
        RWFRZ_EXIT_WATCH_TGID,
        FUTEX_BITSET_MATCH_ANY,
        FLAGS_SHARED,
        FLAGS_CLOCKRT,
        FUTEX_OP_SET,
        FUTEX_OP_ADD,
        FUTEX_OP_OR,
        FUTEX_OP_ANDN,
        FUTEX_OP_XOR,
        FUTEX_OP_CMP_EQ,
        FUTEX_OP_CMP_NE,
        FUTEX_OP_CMP_LT,
        FUTEX_OP_CMP_LE,
        FUTEX_OP_CMP_GT,
        FUTEX_OP_CMP_GE,
        task_futex_mm_id,
        WAITER_POOL,
        HASH_SIZE,
        HASH_HEADS,
        waiter_of,
        FUXWATCH_ADDR,
        FUXWATCH2_ADDR,
        FUXWATCH_DELAY_S,
        FUXWATCH_MM_DONE,
        FUXWATCH1_TGID,
        FUXWATCH2_TGID,
        FUXPARK_WORD,
        FUXPARK_TID,
        FUXPARK_TGID,
        FUXPARK_J,
        fuxwatch_retire_page,
        fuxwatch_retire_all,
        FUXWATCH_OWNER_TID,
        FUXWATCH_OWNER_J,
        fuxwatch_arm_round,
        fuxwatch_page_matches,
        fuxwatch_mm_armed,
        fuxwatch_over_budget,
        fuxwatch_log_write,
        fuxwatch_read_word,
        fuxwatch_take_step,
        fuxwatch_page_watched,
        fuxwatch_read_word_at,
        fuxwatch_addr,
        fuxwatch2_addr,
        rd_word,
        HEAPMAP_ARM,
        OH68_KEY,
        OH68_OWNER_CTX,
        FUXWATCH_OWNER_TASK,
        oh68_fs_base,
        oh68_ctx_of,
        oh68_latch_owner_task,
        futex_stuck_census,
        futex_hash,
        futex_wake,
        futex_wake_in_mm,
        futex_wait,
        futex_wait_timeout,
        futex_cleanup,
        futex_wait_bitset,
        futex_parse_timeout_ns,
        futex_wake_bitset,
        futex_to_flags,
        futex_requeue,
        FUTEX_OP_OPARG_SHIFT,
        futex_wake_op_decode,
        futex_wake_op,
        do_futex,
        sys_futex_handler,
        RWFRZ_UADDR,
        RWFRZ_OPLOG,
        apppark_rets_public,
        apppark_rets,
        rwfz_emit_prio,
        rwfz_stack_frames,
    };
    // compiled in the core only with its futex tracer feature
    #[cfg(feature = "debug-futex-trace")]
    pub use rux::sync::hotpath::debug_dump_futex_state;
    pub use crate::futex_diag::*;
}

// the guest-test module that followed the files (the framework's
// tests/mod.rs row is gated with the move note); the framework's
// test counters + assert macro re-exported at the crate root, where
// the test module's `super::` imports expect them (the service-test
// seam contract).
#[cfg(feature = "unit-test")]
#[path = "../../core/src/tests/services/futex_test.rs"]
pub mod futex_test;
#[cfg(feature = "unit-test")]
pub use rux::tests::{test_pass, test_fail, test_group_start};
#[cfg(feature = "unit-test")]
pub use rux::test_assert_eq;

/// The boot-time registration (bin calls this before
/// rust_main_impl): the SyncDiagHooks table's rows. OnceSlot-only — no
/// Spinlock, no per-CPU state (the x86_64 unit-test kernel lesson).
pub fn register_all() {
    rux::api::sync::sync_diag_register_hooks(
        rux::api::sync::SyncDiagHooks {
            fxdv_latch_devmgr: futex_diag::fxdv_latch_devmgr,
            rwfz_exit_watch_tgid: futex_diag::rwfz_exit_watch_tgid,
            rwfrz_tgid_frozen: futex_diag::rwfrz_tgid_frozen,
            note_task_unlink: futex_diag::note_task_unlink,
            kill_dump_tgid: futex_diag::kill_dump_tgid,
            sqwalk_resume: futex_diag::sqwalk_resume,
            ffrt_mutex_heap_map_done: futex_diag::ffrt_mutex_heap_map_done,
            ffrt_mutex_heap_map_rearm: futex_diag::ffrt_mutex_heap_map_rearm,
            ffrt_mutex_heap_map: futex_diag::ffrt_mutex_heap_map,
            fuxwatch_parse: futex_diag::fuxwatch_parse,
            fuxwatch2_parse: futex_diag::fuxwatch2_parse,
            malloc_holder_scan: futex_diag::malloc_holder_scan,
            ctd_decode: futex_diag::ctd_decode,
            fuxpark_latch: futex_diag::fuxpark_latch,
            rwfrz_reader_scan: futex_diag::rwfrz_reader_scan,
        },
    );
}

/// The guest-test entry (make test): the framework's run_all_tests
/// invokes this through the SERVICE-TEST SEAM (bin registers it at
/// boot). Reporting goes through the framework's counters, so pass/fail
/// lands in the same totals as the in-framework tests.
#[cfg(feature = "unit-test")]
pub fn run_unit_tests() {
    futex_test::test_futex();
}
