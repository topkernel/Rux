//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! SysV semaphore blocking-contract tests (R40).

//! Blocking contract of semop (R40): an operation that cannot proceed
//! and does NOT carry IPC_NOWAIT must BLOCK until a concurrent post
//! satisfies it — it must never answer EAGAIN. EAGAIN belongs to
//! IPC_NOWAIT operations (and to semtimedop deadline expiry). Regression
//! guard for the lmbench lat_sem failure: the writer's blocking semop
//! raced the reader's concurrent post between try_apply_semops and the
//! blocking-op scan, the scan found nothing, and the syscall surfaced
//! EAGAIN — killing the writer and stranding the reader
//! (run/lat_sem-before.log: "LMBENCH TIMEOUT lat_sem").
//!
//! Both sides drive the production loop through semop_kernel (same
//! validation, same apply/block core as the syscall, sops built in the
//! kernel: the unit-test boot task has no user mm to uaccess through).
//! The poster runs in a kernel thread — the helper kthread has no user
//! address space either, so semop_kernel is its only way to post.
use crate::ipc::sysv_sem::{semop_kernel, sys_semctl, sys_semget, SemBuf};
use alloc::format;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, Ordering};
use super::{test_pass, test_fail, test_skip, test_group_start};

// semctl commands (uapi sem.h)
const GETVAL: u64 = 12;
const GETNCNT: u64 = 14;
const GETZCNT: u64 = 15;
const SETVAL: u64 = 16;
const IPC_RMID: u64 = 0;
const IPC_CREAT: i64 = 0o1000;

// sem_flg bits
const IPC_NOWAIT: u16 = 0o4000;

// errno
const EAGAIN: i64 = 11;

/// Helper-thread plan. Static because the boot task may already be
/// asleep on the semaphore while the helper reads it.
struct PostPlan {
    /// Set the helper operates on.
    semid: AtomicI32,
    /// 0 → wait for GETNCNT==1 (P-waiter), 1 → wait for GETZCNT==1
    /// (Z-waiter) before posting.
    wait_zcnt: AtomicU8,
    /// The helper really observed the main task registered as a waiter —
    /// proof the op took the NEED_BLOCK path (ncnt/zcnt are incremented
    /// before sleeping), not an immediate success.
    observed: AtomicBool,
}

static PLAN: PostPlan = PostPlan {
    semid: AtomicI32::new(-1),
    wait_zcnt: AtomicU8::new(0),
    observed: AtomicBool::new(false),
};

extern "C" fn sem_post_helper(_arg: *mut core::ffi::c_void) -> i32 {
    let semid = PLAN.semid.load(Ordering::Acquire);
    let counter_cmd = if PLAN.wait_zcnt.load(Ordering::Acquire) == 1 {
        GETZCNT
    } else {
        GETNCNT
    };
    // Wait (bounded busy spin: timer interrupts are off in the unit-test
    // boot) until the main task is registered as a waiter. In the normal
    // flow the helper only gets CPU time after the main task slept, so
    // the very first poll sees 1; the bound only bites if the op never
    // blocks (then the post still lets the caller finish).
    let mut observed = false;
    let mut spins: u64 = 0;
    while spins < 2_000_000 {
        let n = sys_semctl([semid as u64, 0, counter_cmd, 0, 0, 0]);
        if n == 1 {
            observed = true;
            break;
        }
        if n < 0 {
            // Set vanished: post anyway so the caller can finish.
            break;
        }
        spins += 1;
    }
    PLAN.observed.store(observed, Ordering::Release);
    // The post: a plain V through the same apply path a userspace
    // semop(+1) takes (atomic apply + wake the set's wait queue).
    let sop = SemBuf {
        sem_num: 0,
        sem_op: 1,
        sem_flg: 0,
    };
    semop_kernel(semid, &[sop]);
    0
}

/// Spawn the post helper for `semid`. Returns false when the kthread
/// could not be created (the caller must then NOT issue the blocking op
/// — nobody would post it).
fn spawn_helper(semid: i64, wait_zcnt: bool) -> bool {
    PLAN.semid.store(semid as i32, Ordering::Release);
    PLAN.wait_zcnt.store(if wait_zcnt { 1 } else { 0 }, Ordering::Release);
    PLAN.observed.store(false, Ordering::Release);
    crate::process::kthread::kthread_run(sem_post_helper, core::ptr::null_mut(), "semtest_post")
        .is_some()
}

/// One private set per sub-test (IPC_PRIVATE always creates fresh).
fn fresh_set() -> i64 {
    sys_semget([0, 1, IPC_CREAT as u64 | 0o600, 0, 0, 0])
}

fn rmid(semid: i64) {
    sys_semctl([semid as u64, 0, IPC_RMID, 0, 0, 0]);
}

fn sop(sem_op: i16, sem_flg: u16) -> SemBuf {
    SemBuf {
        sem_num: 0,
        sem_op,
        sem_flg,
    }
}

fn getval(semid: i64) -> i64 {
    sys_semctl([semid as u64, 0, GETVAL, 0, 0, 0])
}

pub fn test_sysv_sem_block() {
    test_group_start("sysv sem blocking contract");

    // ---- Sub-test 1: EAGAIN only with IPC_NOWAIT ----
    // P(-1) on a zero semaphore with IPC_NOWAIT: must fail EAGAIN
    // immediately. The same op WITHOUT the flag must never return EAGAIN
    // (pinned by the sub-tests below).
    {
        let semid = fresh_set();
        if semid < 0 {
            test_fail("fresh set", "semget failed");
            return;
        }
        let ret = semop_kernel(semid as i32, &[sop(-1, IPC_NOWAIT)]);
        if ret == -EAGAIN {
            test_pass("IPC_NOWAIT P on 0 -> EAGAIN");
        } else {
            test_fail("IPC_NOWAIT P on 0", &format!("ret={} want {}", ret, -EAGAIN));
        }
        // Sanity: a satisfiable op still succeeds and is applied.
        let ret = semop_kernel(semid as i32, &[sop(1, 0)]);
        let val = getval(semid);
        if ret == 0 && val == 1 {
            test_pass("V applied (GETVAL==1)");
        } else {
            test_fail("V applied", &format!("ret={} val={}", ret, val));
        }
        rmid(semid);
    }

    // ---- Sub-test 2: blocking P completes after a cross-task post ----
    // P(-1) with flg=0 on a zero semaphore: must block, then return 0
    // once the helper posts. Any EAGAIN here is THE bug (lat_sem).
    {
        let semid = fresh_set();
        if semid < 0 {
            test_fail("fresh set (P)", "semget failed");
            return;
        }
        if !spawn_helper(semid, false) {
            test_skip("blocking P completes", "kthread spawn failed");
            rmid(semid);
            return;
        }
        let ret = semop_kernel(semid as i32, &[sop(-1, 0)]);
        let observed = PLAN.observed.load(Ordering::Acquire);
        let val = getval(semid);
        if ret == 0 && observed && val == 0 {
            test_pass("blocking P completes after post (no EAGAIN)");
        } else if ret == -EAGAIN {
            test_fail("blocking P completes", "EAGAIN for non-IPC_NOWAIT op (the bug)");
        } else {
            test_fail(
                "blocking P completes",
                &format!("ret={} observed={} val={}", ret, observed, val),
            );
        }
        rmid(semid);
    }

    // ---- Sub-test 3: accumulated multi-op set blocks, no spurious EAGAIN ----
    // [{-1},{-1}] on value 1: the SECOND -1 blocks against the working
    // value 0 (accumulated semantics). The old blocking-op scan re-read
    // the raw value for every op, decided "nothing blocks" with NO
    // concurrency at all, and answered EAGAIN. Must block, then succeed
    // after the helper posts a second unit.
    {
        let semid = fresh_set();
        if semid < 0 {
            test_fail("fresh set (2xP)", "semget failed");
            return;
        }
        sys_semctl([semid as u64, 0, SETVAL, 1, 0, 0]);
        if !spawn_helper(semid, false) {
            test_skip("accumulated 2xP blocks", "kthread spawn failed");
            rmid(semid);
            return;
        }
        let ret = semop_kernel(semid as i32, &[sop(-1, 0), sop(-1, 0)]);
        let observed = PLAN.observed.load(Ordering::Acquire);
        let val = getval(semid);
        if ret == 0 && observed && val == 0 {
            test_pass("accumulated 2xP blocks then applies (no EAGAIN)");
        } else if ret == -EAGAIN {
            test_fail("accumulated 2xP", "EAGAIN for non-IPC_NOWAIT op (the bug)");
        } else {
            test_fail(
                "accumulated 2xP",
                &format!("ret={} observed={} val={}", ret, observed, val),
            );
        }
        rmid(semid);
    }

    // ---- Sub-test 4: Z-wait (sem_op==0) blocks until zero ----
    // op {0, 0} on value 1: blocks; helper's V is NOT enough to satisfy
    // it (it raises the value) — use semctl(SETVAL 0), the documented
    // semctl release for Z-waiters, as the second release path.
    {
        let semid = fresh_set();
        if semid < 0 {
            test_fail("fresh set (Z)", "semget failed");
            return;
        }
        sys_semctl([semid as u64, 0, SETVAL, 1, 0, 0]);
        PLAN.semid.store(semid as i32, Ordering::Release);
        PLAN.wait_zcnt.store(1, Ordering::Release);
        PLAN.observed.store(false, Ordering::Release);
        let spawned = crate::process::kthread::kthread_run(
            sem_zrelease_helper,
            core::ptr::null_mut(),
            "semtest_zrel",
        )
        .is_some();
        if !spawned {
            test_skip("Z-wait blocks", "kthread spawn failed");
            rmid(semid);
            return;
        }
        let ret = semop_kernel(semid as i32, &[sop(0, 0)]);
        let observed = PLAN.observed.load(Ordering::Acquire);
        if ret == 0 && observed {
            test_pass("Z-wait blocks until zero (no EAGAIN)");
        } else if ret == -EAGAIN {
            test_fail("Z-wait", "EAGAIN for non-IPC_NOWAIT op (the bug)");
        } else {
            test_fail("Z-wait", &format!("ret={} observed={}", ret, observed));
        }
        rmid(semid);
    }
}

/// Z-wait variant of the helper: waits for GETZCNT==1, then releases
/// with semctl(SETVAL 0) — the value going to 0 is what satisfies a
/// Z-waiter (a V would move it the wrong way).
extern "C" fn sem_zrelease_helper(_arg: *mut core::ffi::c_void) -> i32 {
    let semid = PLAN.semid.load(Ordering::Acquire);
    let mut observed = false;
    let mut spins: u64 = 0;
    while spins < 2_000_000 {
        let n = sys_semctl([semid as u64, 0, GETZCNT, 0, 0, 0]);
        if n == 1 {
            observed = true;
            break;
        }
        if n < 0 {
            break;
        }
        spins += 1;
    }
    PLAN.observed.store(observed, Ordering::Release);
    sys_semctl([semid as u64, 0, SETVAL, 0, 0, 0]);
    0
}
