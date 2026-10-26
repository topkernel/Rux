//! Leak-origin probe (dfx=leakhunt) — names the acquire site of a leaked
//! PREEMPT_OFFSET.
//!
//! The sc261-263 OH boots (and the fleet's riscv monkey lanes) show tasks
//! carrying a leaked preempt_count of exactly 1 across park/wake cycles
//! (PARKED-LEAK; multi-second WAKE-STRANDED incumbents with pc=1/2). The
//! static audits found no unpaired add/sub pair, no underflow (no double
//! drop), no discipline-bypassing switch (SWITCH-COUNT-LEAK stayed at 0
//! all boot) and no 0→nonzero parked transition (PARKED-WILDWRITE never
//! fired) — the producer is a runtime shape the existing probes can only
//! see seconds after the fact.
//!
//! This instrument closes the gap from the other side: it records, for
//! every task, the call site of the OUTERMOST PREEMPT_OFFSET acquire
//! (the 0→1 transition — the one whose loss is the leak), and trips a
//! report the moment a task crosses back into USERSPACE with a nonzero
//! count. A user-mode return with preempt_count != 0 is a definitive
//! leak: no in-flight IRQ can be booked on the task at that point, and
//! every guard the syscall path took must have been dropped. The report
//! carries the recorded acquire site (file:line of the guard
//! construction) — the producer is named directly, not inferred.
//!
//! Costs when armed (dfx=leakhunt): one side-table store per outermost
//! guard acquire and one preempt_count load per user-mode trap return.
//! OFF by default (the switch mirrors into ENABLED); the disabled-path
//! cost is one relaxed load per outermost acquire.
//!
//! Print discipline: direct console (the trip can fire from IRQ-laden
//! contexts), one report per leak episode per task (the latch re-arms on
//! the next 0→1 acquire), total budget 400 lines per boot.

use core::panic::Location;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// Mirror of the dfx=leakhunt runtime switch (set from dfx/switches.rs).
pub static ENABLED: AtomicBool = AtomicBool::new(false);

/// Total print budget for the boot (reports + parked-site lines).
static BUDGET: AtomicU32 = AtomicU32::new(400);

const SLOTS: usize = 2048;
const SLOT_MASK: usize = SLOTS - 1;

struct LoSlot {
    task: AtomicUsize,
    /// `&'static Location` of the most recent PREEMPT_OFFSET acquire.
    site: AtomicUsize,
    /// jiffies at that acquire.
    jif: AtomicU64,
    /// preempt_count value immediately AFTER that acquire (1 = outermost).
    cnt_after: AtomicI32,
    /// One report per leak episode (cleared on each new acquire).
    reported: AtomicBool,
}

static TABLE: [LoSlot; SLOTS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: LoSlot = LoSlot {
        task: AtomicUsize::new(0),
        site: AtomicUsize::new(0),
        jif: AtomicU64::new(0),
        cnt_after: AtomicI32::new(0),
        reported: AtomicBool::new(false),
    };
    [ZERO; SLOTS]
};

#[inline]
fn slot_of(task: *mut crate::process::Task) -> &'static LoSlot {
    let a = task as usize;
    &TABLE[(a >> 6 ^ a >> 12) & SLOT_MASK]
}

/// Record a PREEMPT_OFFSET acquire (last wins). Called from
/// `preempt_count_add` when the probe is armed.
///
/// v2 instant tripwire: an acquire booked on a task that owns no CPU
/// (`!on_cpu`) or is not in RUNNING state is a CROSS-TASK BOOKING — the
/// producer of the parked-leak family (sc264: every parked victim's
/// recorded site is __schedule's own GRQ acquire, sched.rs:2152 — an
/// execution context whose `current()` resolves to a task that is
/// simultaneously parked). At catch time we print the booking CPU's
/// sched-slot vs tp divergence, the slot's last writer tag, and the
/// victim's ownership state — the live adjudicator between stale-tp and
/// double-run shapes. Skips pid 0 (idle protocol), PF_EXITING tasks
/// (exit-path bare adds park by design) and the first 500 jiffies
/// (pre-sched boot task).
#[inline]
pub fn note_acquire(
    task: *mut crate::process::Task,
    loc: &'static Location,
    cnt_after: i32,
) {
    let slot = slot_of(task);
    slot.task.store(task as usize, Ordering::Release);
    slot.site.store(loc as *const Location as usize, Ordering::Release);
    slot.jif
        .store(crate::drivers::timer::get_jiffies(), Ordering::Release);
    slot.cnt_after.store(cnt_after, Ordering::Release);
    slot.reported.store(false, Ordering::Release);
    // SAFETY: diagnostic reads of stable Task fields.
    let (pid, on_cpu, st_running, exiting) = unsafe {
        (
            (*task).pid(),
            (*task).on_cpu(),
            (*task).state().is_running(),
            (*task).ti_flags()
                & crate::process::task::task_flags::TaskFlags::PF_EXITING
                    .bits()
                != 0,
        )
    };
    if pid != 0 && !exiting && (!on_cpu || !st_running) {
        catch_booked_on_nonrunning(task, loc, cnt_after, on_cpu, st_running);
    }
}

/// Rate gate for the booked-on-nonrunning catch: one per 20 jiffies.
fn catch_budget_ok() -> bool {
    static LAST: AtomicU64 = AtomicU64::new(0);
    static N: AtomicU32 = AtomicU32::new(0);
    if N.load(Ordering::Acquire) >= 200 {
        return false;
    }
    let now = crate::drivers::timer::get_jiffies();
    if now < 500 {
        return false; // pre-sched boot phase
    }
    let last = LAST.load(Ordering::Acquire);
    if now.wrapping_sub(last) < 20 {
        return false;
    }
    LAST.compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
        && N.fetch_add(1, Ordering::AcqRel) < 200
}

fn catch_booked_on_nonrunning(
    task: *mut crate::process::Task,
    loc: &'static Location,
    cnt_after: i32,
    on_cpu: bool,
    st_running: bool,
) {
    if !catch_budget_ok() {
        return;
    }
    use crate::console::putchar_no_lock as putchar;
    fn ph(mut v: u64) {
        use crate::console::putchar_no_lock as putchar;
        for sh in (0..16).rev() {
            let n = ((v >> (sh * 4)) & 0xF) as u8;
            putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
        }
    }
    fn pd(mut v: u64) {
        use crate::console::putchar_no_lock as putchar;
        let mut buf = [0u8; 20];
        let mut n = 0;
        if v == 0 {
            buf[0] = b'0';
        } else {
            while v > 0 && n < buf.len() {
                buf[n] = b'0' + (v % 10) as u8;
                v /= 10;
                n += 1;
            }
        }
        for i in (0..n).rev() {
            putchar(buf[i]);
        }
    }
    const M: &[u8] = b"\nLEAK-ORIGIN booked-on-nonrunning cpu=";
    for &c in M {
        putchar(c);
    }
    let cpu = crate::arch::cpu_id() as usize;
    ph(cpu as u64);
    // SAFETY: diagnostic reads of stable Task fields.
    unsafe {
        const M2: &[u8] = b" victim_pid=";
        for &c in M2 {
            putchar(c);
        }
        pd((*task).pid() as u64);
        const M3: &[u8] = b" tp_state=";
        for &c in M3 {
            putchar(c);
        }
        ph((*task).state().bits() as u64);
        const M4: &[u8] = b" on_cpu=";
        for &c in M4 {
            putchar(c);
        }
        pd(on_cpu as u64);
        const M5: &[u8] = b" running=";
        for &c in M5 {
            putchar(c);
        }
        pd(st_running as u64);
        const M6: &[u8] = b" claim=";
        for &c in M6 {
            putchar(c);
        }
        // Negative claims encode pick state; print the raw word.
        ph((*task).running_on_cpu.load(Ordering::Acquire) as u32 as u64);
        const M7: &[u8] = b" pick_st=";
        for &c in M7 {
            putchar(c);
        }
        ph((*task).pick_state() as u64);
    }
    // The booking CPU's sched slot vs the tp task — the R55 divergence
    // at catch time — plus the slot's last writer tag (1=pick 2=R56d
    // 3=R56c 4=steal 5=abort-republish).
    const M8: &[u8] = b" slot_pid=";
    for &c in M8 {
        putchar(c);
    }
    let sl = crate::sched::sched::cpu_state(cpu).current;
    if sl.is_null() {
        for &c in b"nil" {
            putchar(c);
        }
    } else {
        // SAFETY: init-time valid slot pointer; scalar field read.
        pd(unsafe { (*sl).pid() } as u64);
    }
    const M9: &[u8] = b" slotw=";
    for &c in M9 {
        putchar(c);
    }
    ph(crate::sched::sched::SLOT_WRITER[cpu]
        .load(Ordering::Relaxed) as u64);
    const M9B: &[u8] = b" slotw_task=";
    for &c in M9B {
        putchar(c);
    }
    ph(crate::sched::sched::SLOT_WRITER_TASK[cpu]
        .load(Ordering::Relaxed));
    const M10: &[u8] = b" cnt_after=";
    for &c in M10 {
        putchar(c);
    }
    ph(cnt_after as u32 as u64);
    const M11: &[u8] = b" site=";
    for &c in M11 {
        putchar(c);
    }
    for &c in loc.file().as_bytes() {
        putchar(c);
    }
    putchar(b':');
    pd(loc.line() as u64);
    putchar(b'\n');
}

/// User-mode return tripwire. Call at the trap-handler tail when the
/// frame returns to userspace: preempt_count MUST be 0 there. Direct
/// console, latched per episode, budgeted.
pub fn check_user_return() {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let Some(task) = crate::sched::current() else {
        return;
    };
    // SAFETY: current() pins a live task; scalar field reads.
    let pc = unsafe { (*task).preempt_count() };
    if pc == 0 {
        return;
    }
    let slot = slot_of(task);
    if slot.task.load(Ordering::Acquire) != task as *mut _ as usize {
        return;
    }
    if slot.reported.swap(true, Ordering::AcqRel) {
        return;
    }
    emit(b"\nLEAK-ORIGIN user-return", task, pc);
}

/// Append the recorded site for a PARKED-LEAK victim (called from the
/// sched.rs parkwalk printer, already rate-gated there).
///
/// v3: the walk classifies "parked" via the SCHED PER_CPU slots; a task
/// can be current on NO sched slot yet still EXECUTING (tp names it) —
/// the R55 stale-slot family would then read as a phantom parked-leak
/// (a legit live guard on a running task). The report now scans the
/// ARCH tp slots: `tpcpu=N` (plus the arch-slot count) means the victim
/// is actually running on N and the parkwalk's parked classification is
/// the artifact; no tpcpu = genuinely parked, and the +1 is real.
pub fn report_parked_site(task: *mut crate::process::Task) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    use crate::console::putchar_no_lock as putchar;
    fn pd(mut v: u64) {
        use crate::console::putchar_no_lock as putchar;
        let mut buf = [0u8; 20];
        let mut n = 0;
        if v == 0 {
            buf[0] = b'0';
            n = 1;
        } else {
            while v > 0 && n < buf.len() {
                buf[n] = b'0' + (v % 10) as u8;
                v /= 10;
                n += 1;
            }
        }
        for i in (0..n).rev() {
            putchar(buf[i]);
        }
    }
    // Scan the arch tp slots for the victim.
    let mut tpcpu: i32 = -1;
    let mut tppc: i32 = 0;
    for c in 0..crate::config::MAX_CPUS {
        let tp = crate::arch::cpu::get_thread_id_of(c) as *mut crate::process::Task;
        if tp == task {
            tpcpu = c as i32;
            // SAFETY: diagnostic field read.
            tppc = unsafe { (*tp).preempt_count() };
            break;
        }
    }
    const A: &[u8] = b"\nLEAK-ORIGIN parked-adjacent tp=";
    for &c in A {
        putchar(c);
    }
    pd(tpcpu as u64);
    if tpcpu >= 0 {
        const B: &[u8] = b" tpc_pc=";
        for &c in B {
            putchar(c);
        }
        pd(tppc as u64);
    }
    emit(b" LEAK-ORIGIN parked", task, unsafe { (*task).preempt_count() });
}

/// Append the recorded site for the tick-side PREEMPT-LEAK incumbent
/// (called from the scheduler_tick tripwire's print block).
pub fn report_incumbent_site(task: *mut crate::process::Task) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    emit(b"\nLEAK-ORIGIN running", task, unsafe { (*task).preempt_count() });
}

/// One report line: tag, pid, pc, site=<file>:<line> cnt_after=<n>
/// age=<jiffies> for the recorded acquire.
fn emit(tag: &[u8], task: *mut crate::process::Task, pc: i32) {
    use crate::console::putchar_no_lock as putchar;
    let b = BUDGET.load(Ordering::Acquire);
    if b == 0 {
        return;
    }
    BUDGET
        .compare_exchange(b, b - 1, Ordering::AcqRel, Ordering::Acquire)
        .ok();
    fn put_i64(mut v: i64) {
        use crate::console::putchar_no_lock as putchar;
        if v < 0 {
            putchar(b'-');
            v = v.wrapping_neg();
        }
        let mut buf = [0u8; 20];
        let mut n = 0;
        if v == 0 {
            buf[0] = b'0';
            n = 1;
        } else {
            while v > 0 && n < buf.len() {
                buf[n] = b'0' + (v % 10) as u8;
                v /= 10;
                n += 1;
            }
        }
        for i in (0..n).rev() {
            putchar(buf[i]);
        }
    }
    let slot = slot_of(task);
    let now = crate::drivers::timer::get_jiffies();
    for &c in tag {
        putchar(c);
    }
    for &c in b" pid=" {
        putchar(c);
    }
    put_i64(unsafe { (*task).pid() } as i64);
    for &c in b" pc=" {
        putchar(c);
    }
    put_i64(pc as i64);
    let site = slot.site.load(Ordering::Acquire) as *const Location;
    if slot.task.load(Ordering::Acquire) == task as usize && !site.is_null() {
        // SAFETY: stored from a &'static Location (see site_of).
        let loc = unsafe { &*site };
        for &c in b" site=" {
            putchar(c);
        }
        for &c in loc.file().as_bytes() {
            putchar(c);
        }
        putchar(b':');
        put_i64(loc.line() as i64);
    } else {
        for &c in b" site=?" {
            putchar(c);
        }
    }
    for &c in b" cnt_after=" {
        putchar(c);
    }
    put_i64(slot.cnt_after.load(Ordering::Acquire) as i64);
    for &c in b" age_jif=" {
        putchar(c);
    }
    put_i64(now.wrapping_sub(slot.jif.load(Ordering::Acquire)) as i64);
    putchar(b'\n');
}
