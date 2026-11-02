//! Context-switch / wake / IPC-syscall cost anatomy (closegap lane,
//! cmdline `cctx=1`).
//!
//! lmbench gap attribution probe for the four worst latency rows:
//! lat_sem (12x), lat_unix (3.9x), lat_ctx (3.4-5.1x), lat_pipe (3.3x).
//! Each row shares the same two bodies — the block/wake context-switch
//! loop and a per-syscall body — so this probe brackets BOTH, per phase:
//!
//! - __schedule (prev's frame): guards (pre), GRQ lock wait, runtime
//!   accounting, prev dequeue/requeue, pick_next_task, publish/unlock.
//! - context_switch wrapper: FPU save, CR3 switch, FS/GS MSR swaps, the
//!   switch asm itself (stamp written by the INCOMING task after
//!   __switch_to returns), FPU restore.
//! - Task::wake_up_impl: select_task_rq, the locked RUNNING+enqueue
//!   section, the idle-claim/resched-IPI dispatch.
//! - resched_cpu: the actual IPI send.
//! - sys_semtimedop: prelude, each try_apply_semops, the blocking park.
//! - pipe read/write, unix stream send/recv: whole-op brackets.
//!
//! Discipline: same contract as fsw/syscost — own cmdline key (`cctx=1`),
//! lazy-armed after cmdline init, NS sums with a count each (averages at
//! dump), zero traffic when off (one Relaxed load per stamp call), dump
//! from the shutdown cascade (every probe rig ends in reboot(2)).
//!
//! Read numbers as upper bounds: each bracket pair adds ~20-30 ns of its
//! own clock-read cost to its phase; the ranking and aggregate split are
//! solid.

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use core::sync::atomic::{AtomicU8, AtomicU64, Ordering};

// === __schedule phases (prev's frame) =====================================
/// __schedule entries past the idle fast-path guard.
pub static SCH_N: AtomicU64 = AtomicU64::new(0);
/// entry -> just before the GRQ acquire (prev-resolution + heal guards).
pub static SCH_PRE_NS: AtomicU64 = AtomicU64::new(0);
/// GRQ lock acquire time (contended spins land here).
pub static SCH_LOCKWAIT_NS: AtomicU64 = AtomicU64::new(0);
/// Runtime accounting (update_curr / DL consumption).
pub static SCH_ACCT_NS: AtomicU64 = AtomicU64::new(0);
/// prev dequeue (sleeper) / requeue (still runnable).
pub static SCH_DEQREQ_NS: AtomicU64 = AtomicU64::new(0);
/// pick_next_task (class queues + heal machinery).
pub static SCH_PICK_NS: AtomicU64 = AtomicU64::new(0);
/// current-slot publish + schedstat + unlock.
pub static SCH_PUB_NS: AtomicU64 = AtomicU64::new(0);
/// Real context switches (next != prev) — the divisor for CW_* rows.
pub static CTX_N: AtomicU64 = AtomicU64::new(0);

// === context_switch wrapper ===============================================
pub static CW_N: AtomicU64 = AtomicU64::new(0);
/// fpu_save_for_switch (fxsave/xsave into the outgoing task).
pub static CW_FPUSAVE_NS: AtomicU64 = AtomicU64::new(0);
/// switch_mm / CR3 write (counted separately: CW_MM_N).
pub static CW_MM_N: AtomicU64 = AtomicU64::new(0);
pub static CW_MM_NS: AtomicU64 = AtomicU64::new(0);
/// FS/GS base MSR read/write pair set.
pub static CW_MSR_NS: AtomicU64 = AtomicU64::new(0);
/// __switch_to asm proper: stamp before the call (outgoing frame), stamp
/// after it returns (incoming frame) — the true register/stack switch.
pub static CW_ASM_NS: AtomicU64 = AtomicU64::new(0);
/// restore_fpu of the incoming task.
pub static CW_FPUREST_NS: AtomicU64 = AtomicU64::new(0);

/// Per-CPU switch-phase stamp (arch context_switch): the OUTGOING task
/// stores its pre-asm stamp, the INCOMING task on the same CPU (IRQs are
/// off across the switch) reads it back and closes the ASM bracket —
/// prev's frame is frozen mid-__switch_to, so the stamp cannot live in a
/// local.
pub static SW_STAMP: [AtomicU64; crate::config::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::config::MAX_CPUS];

// === wake path =============================================================
/// Task::wake_up_impl sleeping-target wakes.
pub static WK_N: AtomicU64 = AtomicU64::new(0);
/// select_task_rq (sched-class CPU choice).
pub static WK_SEL_NS: AtomicU64 = AtomicU64::new(0);
/// wake_up_enqueue_opt (GRQ lock + RUNNING transition + enqueue + preempt
/// check). A cross-CPU steer adds its own GRQ acquire inside this window.
pub static WK_ENQ_NS: AtomicU64 = AtomicU64::new(0);
/// idle-claim + resched IPI / preempt_check_wake dispatch.
pub static WK_DISP_NS: AtomicU64 = AtomicU64::new(0);

// === IPI ===================================================================
pub static IPI_N: AtomicU64 = AtomicU64::new(0);
pub static IPI_NS: AtomicU64 = AtomicU64::new(0);

// === semop =================================================================
pub static SM_N: AtomicU64 = AtomicU64::new(0);
/// entry -> first try_apply (copies, perms find, nsems validate).
pub static SM_PRE_NS: AtomicU64 = AtomicU64::new(0);
/// Every try_apply_semops call (count SM_TRY_N).
pub static SM_TRY_N: AtomicU64 = AtomicU64::new(0);
pub static SM_TRY_NS: AtomicU64 = AtomicU64::new(0);
/// Blocking park: find_blocking + register + retry apply + schedule().
pub static SM_BLK_N: AtomicU64 = AtomicU64::new(0);
pub static SM_BLK_NS: AtomicU64 = AtomicU64::new(0);

// === pipe / unix ===========================================================
pub static PW_N: AtomicU64 = AtomicU64::new(0);
pub static PW_NS: AtomicU64 = AtomicU64::new(0);
pub static PR_N: AtomicU64 = AtomicU64::new(0);
pub static PR_NS: AtomicU64 = AtomicU64::new(0);
pub static UW_N: AtomicU64 = AtomicU64::new(0);
pub static UW_NS: AtomicU64 = AtomicU64::new(0);
pub static UR_N: AtomicU64 = AtomicU64::new(0);
pub static UR_NS: AtomicU64 = AtomicU64::new(0);

/// Bracket timestamp (0 when the probe is off — skips the clock read).
#[inline]
pub fn now() -> u64 {
    if on() {
        crate::drivers::timer::monotonic_ns_since_boot()
    } else {
        0
    }
}

/// Close a bracket opened with now() (no-op when t0 == 0, i.e. off).
#[inline]
pub fn add(dst: &AtomicU64, t0: u64) {
    if t0 != 0 {
        dst.fetch_add(
            crate::drivers::timer::monotonic_ns_since_boot().saturating_sub(t0),
            Ordering::Relaxed,
        );
    }
}

/// Bump a count (no-op when off).
#[inline]
pub fn bump(c: &AtomicU64) {
    if on() {
        c.fetch_add(1, Ordering::Relaxed);
    }
}

/// Probe armed? Tri-state exactly like fsw (0 = unresolved, 1 = off, 2 =
/// on); never cache a verdict before the cmdline exists.
static ARMED8: AtomicU8 = AtomicU8::new(0);

#[inline]
pub fn on() -> bool {
    match ARMED8.load(Ordering::Relaxed) {
        2 => true,
        1 => false,
        _ => {
            let v = match crate::cmdline::get_cmdline() {
                None => return false,
                Some(_) => crate::cmdline::get_param("cctx").as_deref() == Some("1"),
            };
            ARMED8.store(if v { 2 } else { 1 }, Ordering::Relaxed);
            crate::pr_warn!(
                "cctx: {} (cmdline cctx param = {:?})",
                if v { "armed" } else { "off" },
                crate::cmdline::get_param("cctx")
            );
            v
        }
    }
}

fn v(c: &AtomicU64) -> u64 {
    c.load(Ordering::Relaxed)
}

fn avg(sum: &AtomicU64, n: &AtomicU64) -> u64 {
    let n = v(n);
    if n == 0 {
        0
    } else {
        v(sum) / n
    }
}

/// Print the counter snapshot (called from the shutdown cascade; every
/// cctx probe rig ends with reboot(2), so the numbers ride the serial).
pub fn dump() {
    if !on() {
        return;
    }
    let secs = crate::drivers::timer::get_jiffies() / crate::config::KERNEL_HZ as u64;
    crate::pr_warn!(
        // Schedule-side anatomy (all NS sums; averages = sum / leftmost
        // count). SCH rows divide by SCH_N, CW rows by CTX_N.
        "CCTX1 t={}s sch n={} pre={} lock={} acct={} deq={} pick={} pub={} ctxsws={}",
        secs,
        v(&SCH_N),
        avg(&SCH_PRE_NS, &SCH_N),
        avg(&SCH_LOCKWAIT_NS, &SCH_N),
        avg(&SCH_ACCT_NS, &SCH_N),
        avg(&SCH_DEQREQ_NS, &SCH_N),
        avg(&SCH_PICK_NS, &SCH_N),
        avg(&SCH_PUB_NS, &SCH_N),
        v(&CTX_N),
    );
    crate::pr_warn!(
        // context_switch anatomy: fpu save / mm (avg over switches THAT
        // wrote CR3: CW_MM_N) / msr / asm / fpu restore.
        "CCTX2 t={}s csw n={} fpusave={} mm={}/{} (n={}, avg={}) msr={} asm={} fpurest={}",
        secs,
        v(&CW_N),
        avg(&CW_FPUSAVE_NS, &CW_N),
        v(&CW_MM_NS),
        avg(&CW_MM_NS, &CTX_N),
        v(&CW_MM_N),
        avg(&CW_MM_NS, &CW_MM_N),
        avg(&CW_MSR_NS, &CW_N),
        avg(&CW_ASM_NS, &CW_N),
        avg(&CW_FPUREST_NS, &CW_N),
    );
    crate::pr_warn!(
        "CCTX3 t={}s wake n={} sel={} enq={} disp={} ipi n={} avg={}",
        secs,
        v(&WK_N),
        avg(&WK_SEL_NS, &WK_N),
        avg(&WK_ENQ_NS, &WK_N),
        avg(&WK_DISP_NS, &WK_N),
        v(&IPI_N),
        avg(&IPI_NS, &IPI_N),
    );
    crate::pr_warn!(
        "CCTX4 t={}s sem n={} pre={} try n={} avg={} blk n={} avg={}",
        secs,
        v(&SM_N),
        avg(&SM_PRE_NS, &SM_N),
        v(&SM_TRY_N),
        avg(&SM_TRY_NS, &SM_TRY_N),
        v(&SM_BLK_N),
        avg(&SM_BLK_NS, &SM_BLK_N),
    );
    crate::pr_warn!(
        "CCTX5 t={}s pipe w n={} avg={} r n={} avg={} unix w n={} avg={} r n={} avg={}",
        secs,
        v(&PW_N),
        avg(&PW_NS, &PW_N),
        v(&PR_N),
        avg(&PR_NS, &PR_N),
        v(&UW_N),
        avg(&UW_NS, &UW_N),
        v(&UR_N),
        avg(&UR_NS, &UR_N),
    );
}
