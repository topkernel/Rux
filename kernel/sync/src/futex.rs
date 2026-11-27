//! The FUTEX UNION MODULE (the historical `sync::futex` view, now a real
//! file; the sync shrink lane 2026-11-27 turned the lib.rs inline
//! re-export block into this module).
//!
//! Two halves, one public surface:
//! - STAYING names re-exported from the framework's hotpath banner
//!   (the futex mechanism, the fuxwatch trap-called machinery, the
//!   diagnostic statics — every one is named by staying-core code and
//!   cannot move; see the lane's annotated allocation table);
//! - MOVED names defined HERE (this file is the sole copy): the
//!   compiler-verified movable remainder of sync/hotpath.rs's futex
//!   section — three futex opcode consts with no staying-core reader,
//!   the FUXPARK_TID/FUXPARK_J latch statics (ipc/binder_diag reads
//!   them through THIS module at the historical path), and four
//!   helper fns whose bodies touch only published-core items
//!   (pub statics and the safe rux::sync::r#unsafe walkers).
//!
//! Zero behavior change: every item keeps its historical body; the
//! moved fns reach their staying dependencies cross-crate through the
//! published-core rows (the pass-2 recipe: atomics and safe wrappers
//! are safe cross-crate access).

pub use rux::sync::types::{FuxWakeOp, FutexKey, Waiter, FutexChain};

// ---- the staying half (framework hotpath banner) ------------------------
pub use rux::sync::hotpath::{
    FUTEX_WAIT,
    FUTEX_WAKE,
    FUTEX_REQUEUE,
    FUTEX_CMP_REQUEUE,
    FUTEX_WAKE_OP,
    FUTEX_LOCK_PI,
    FUTEX_UNLOCK_PI,
    FUTEX_TRYLOCK_PI,
    FUTEX_WAIT_BITSET,
    FUTEX_WAKE_BITSET,
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
    FUXWATCH_ADDR,
    FUXWATCH_BUDGET,
    FUXWATCH_HITS,
    FUXWATCH2_ADDR,
    FUXWATCH_DELAY_S,
    FUXWATCH_MM_DONE,
    FUXWATCH1_TGID,
    FUXWATCH2_TGID,
    FUXPARK_WORD,
    FUXPARK_TGID,
    fuxwatch_retire_page,
    fuxwatch_retire_all,
    FUXWATCH_OWNER_TID,
    FUXWATCH_OWNER_J,
    fuxwatch_arm_round,
    fuxwatch_page_matches,
    fuxwatch_mm_armed,
    fuxwatch_log_write,
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

// the diagnostic family moved at the sync crate flip (pass 2)
pub use crate::futex_diag::*;

// ---- the moved half (this file is the sole copy) ------------------------

pub const FUTEX_FD: i32 = 2;

pub const FUTEX_WAIT_REQUEUE_PI: i32 = 11;

pub const FUTEX_CMP_REQUEUE_PI: i32 = 12;

/// Jiffies of the first latch (park age reference for the verdict line).
pub static FUXPARK_J: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

pub static FUXPARK_TID: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// Diagnostic (b133/b134 stall hunt): find the live futex-waiter entry of
/// `task`, if any. Returns (uaddr, key.mm, woken). The binder stall report
/// calls this for txn-holder threads parked in futex_wait: a `woken=true`
/// waiter that never resumed indicts the wake→run path, while `woken=false`
/// after minutes indicts the key match (compare key.mm with the task's own
/// current identity) or the userspace releaser.
pub fn waiter_of(task: *mut rux::process::types::Task) -> Option<(usize, usize, bool)> {
    for b in HASH_HEADS.iter() {
        let h = b.lock_irqsave();
        let mut idx = h.head;
        while let Some(i) = idx {
            // SAFETY: slot indices come from chain links under the bucket
            // lock; slots are freed only under both locks, so the entry we
            // copy out stays live for the duration of the read.
            let (entry, next) = {
                let slot = WAITER_POOL[i].lock_nested(&h.proof());
                match *slot {
                    Some(ref w) => (
                        Some((w.key.uaddr, w.key.mm, w.woken, core::ptr::eq(w.task, task))),
                        w.next,
                    ),
                    None => (None, None),
                }
            };
            match entry {
                Some((uaddr, mm, woken, true)) => {
                    drop(h);
                    return Some((uaddr, mm, woken));
                }
                Some(_) => {
                    idx = next;
                }
                None => break,
            }
        }
        drop(h);
    }
    None
}

/// FUTEX_WAIT without a deadline (the musl/glibc common shape).
pub fn futex_wait(uaddr: usize, flags: u32, val: u32, bitset: u32) -> i64 {
    futex_wait_timeout(uaddr, flags, val, bitset, None)
}

/// Budget check for the fuxwatch watch-hound: the watch hound stops
/// arming new single-steps once its hit budget is spent (the hunt-era
/// instruments cap themselves so a pathological loop cannot flood the
/// console; the budget const and the hit counter are published-core
/// rows this file reads cross-crate — atomics, safe).
#[inline]
pub fn fuxwatch_over_budget() -> bool {
    FUXWATCH_HITS.load(core::sync::atomic::Ordering::Relaxed) >= FUXWATCH_BUDGET
}

/// Read the watched word's CURRENT value through the walker (fault
/// context: the word still holds its pre-write value).
pub fn fuxwatch_read_word(root_ppn: u64) -> u32 {
    let addr = FUXWATCH_ADDR.load(core::sync::atomic::Ordering::Relaxed);
    if addr == 0 {
        return 0xffff_ffff;
    }
    // SAFETY: read-only walk of the current mm.
    match rux::sync::r#unsafe::pt_walk(root_ppn, addr) {
        Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
            let kv = rux::arch::mm::memory_layout::phys_to_virt(
                rux::arch::mm::memory_layout::PhysAddr::new((ppn << 12) | (addr & 0xfff)),
            );
            // SAFETY: linear-mapped RAM word.
            rux::sync::r#unsafe::read_volatile_u32(kv.as_usize())
        }
        _ => 0xffff_ffff,
    }
}
