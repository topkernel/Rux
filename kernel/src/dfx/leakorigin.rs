//! Leak-origin probe (dfx=leakhunt) — names the acquire site of a leaked
//! PREEMPT_OFFSET (and, since s14, of a stranded HARDIRQ_OFFSET — the
//! pc=0x10000/0x10001 incumbent face of the fleet's four-faced wedge
//! family; `hsite=` in every report names the trap bracket that booked
//! it).
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
    /// `&'static Location` of the most recent HARDIRQ_OFFSET acquire
    /// (s14: the stranded-0x10000 face — pc=0x10000/0x10001 incumbents
    /// and the 0x10000-grid strand family). With `irq_enter` marked
    /// #[track_caller] this names the trap dispatch site (timer/soft/
    /// ext), so a leaked hardirq bit reports WHICH bracket booked it.
    hsite: AtomicUsize,
    /// jiffies at that HARDIRQ acquire.
    hjif: AtomicU64,
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
        hsite: AtomicUsize::new(0),
        hjif: AtomicU64::new(0),
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
}

/// Record a HARDIRQ_OFFSET acquire (last wins; s14 extension — the
/// stranded-0x10000 face). `loc` is the trap dispatch site when
/// `irq_enter` is #[track_caller]. Kept separate from the PREEMPT
/// shadow so a 0x10001 leak names BOTH producers independently.
#[inline]
pub fn note_hardirq_acquire(
    task: *mut crate::process::Task,
    loc: &'static Location,
    cnt_after: i32,
) {
    let slot = slot_of(task);
    slot.task.store(task as usize, Ordering::Release);
    slot.hsite.store(loc as *const Location as usize, Ordering::Release);
    slot.hjif
        .store(crate::drivers::timer::get_jiffies(), Ordering::Release);
    slot.cnt_after.store(cnt_after, Ordering::Release);
    slot.reported.store(false, Ordering::Release);
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
pub fn report_parked_site(task: *mut crate::process::Task) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    emit(b"\nLEAK-ORIGIN parked", task, unsafe { (*task).preempt_count() });
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
    // s14: the HARDIRQ acquire shadow — names which trap bracket
    // booked the stranded 0x10000 bit on this task, and when.
    let hsite = slot.hsite.load(Ordering::Acquire) as *const Location;
    if slot.task.load(Ordering::Acquire) == task as usize && !hsite.is_null() {
        // SAFETY: stored from a &'static Location (see note_hardirq_acquire).
        let loc = unsafe { &*hsite };
        for &c in b" hsite=" {
            putchar(c);
        }
        for &c in loc.file().as_bytes() {
            putchar(c);
        }
        putchar(b':');
        put_i64(loc.line() as i64);
        for &c in b" hage_jif=" {
            putchar(c);
        }
        put_i64(now.wrapping_sub(slot.hjif.load(Ordering::Acquire)) as i64);
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
