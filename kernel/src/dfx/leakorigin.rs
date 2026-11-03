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
    // ---- phantom-bits (2026-11-01): SOFTIRQ-bit shadow ----
    // The monopolist face of the eighth successor's lb60 dump (pid 552,
    // preempt=0x100, RUNNING, CFS-undefeatable) strands exactly ONE
    // SOFTIRQ_OFFSET whose release never executed anywhere (zero
    // sub=0x100 underflows all boot; SOFTIRQ-BRACKET-LEAK silent). Only
    // two +0x100 bookers exist on this tree (__do_softirq's bracket and
    // SeqLock::write's stats sections) — this shadow names which one
    // booked a stranded bit, and the tick-side orphan scan
    /// (preempt::tick_orphan_scan) reports it while the bit sits.
    /// `&'static Location` of the most recent SOFTIRQ_OFFSET acquire.
    ssite: AtomicUsize,
    /// jiffies at that SOFTIRQ acquire.
    sjif: AtomicU64,
    /// CPU id where it ran.
    scpu: AtomicU32,
    /// 1 = at acquire time this CPU's sched slot named a DIFFERENT task
    /// (booking resolved through tp; the slot is the heal's observer —
    /// divergence means the heal watches the wrong task for this bit).
    smis: AtomicU32,
    /// One report per leak episode (cleared on each new acquire).
    reported: AtomicBool,
    // ---- preempt-hunt (2026-11-01): OUTERMOST acquire tap ----
    // The `site` shadow above is last-acquire-wins: after a leak, any
    // correctly-paired inner guard pair (1->2->1) overwrites it, so the
    // reported site is dominated by the hottest lock family (the four
    // allocator sites) even when the leak's acquire was elsewhere. The
    // OUTERmost acquire (base-field 0->1) whose matching 1->0 release
    // never ran is, by construction, the leak's origin: the base count
    // is still nonzero precisely because THAT release never executed.
    // Inner pairs (1->2->1) do not touch these fields, so the record is
    // stable from the leaking acquire until the count returns to zero
    // (or a heal zeroes it — at which point the WAKE-HEAL report prints
    // this record and names the producer's home site directly).
    /// Task that executed the outermost base acquire (0 = none).
    otask: AtomicUsize,
    /// `&'static Location` of that outermost acquire.
    osite: AtomicUsize,
    /// jiffies at that acquire.
    ojif: AtomicU64,
    /// CPU id where it ran.
    ocpu: AtomicU32,
    /// 1 = at acquire time this CPU's sched slot named a DIFFERENT task
    /// (tp vs slot divergence — the stale-tp / mid-switch window family).
    omis: AtomicU32,
    /// When omis=1: the task pointer the sched slot named at acquire
    /// time (0 = unreadable). Identifies WHICH window the divergent
    /// booking ran in (preempt-hunt).
    oslot: AtomicUsize,
    /// jiffies of the last user-return report for this slot
    /// (preempt-hunt rate gate: one stuck +1 re-reports on every guard
    /// re-acquire; 50-jiffy spacing keeps the episode signature without
    /// flooding the budget).
    urrep_jif: AtomicU64,
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
        ssite: AtomicUsize::new(0),
        sjif: AtomicU64::new(0),
        scpu: AtomicU32::new(0),
        smis: AtomicU32::new(0),
        reported: AtomicBool::new(false),
        otask: AtomicUsize::new(0),
        osite: AtomicUsize::new(0),
        ojif: AtomicU64::new(0),
        ocpu: AtomicU32::new(0),
        omis: AtomicU32::new(0),
        oslot: AtomicUsize::new(0),
        urrep_jif: AtomicU64::new(0),
    };
    [ZERO; SLOTS]
};

#[inline]
fn slot_of(task: *mut crate::process::Task) -> &'static LoSlot {
    let a = task as usize;
    // preempt-hunt: XOR-of-shifts mixes alias — for every shift set there
    // are address strides (observed live: 0x1000000, then 0x2080000)
    // whose contributions XOR to zero, making distinct task structs share
    // slots (bogus ACQUIRE-PAIR-SPLITs, shadowed records). Fibonacci
    // multiplication: no constant stride aliases.
    &TABLE[(a.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 53) & SLOT_MASK]
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
    // Outermost base acquire (base field 0->1): record the leak-origin
    // candidate. `cnt_after & 0xFF == 1` also matches base transitions
    // made under a live softirq field (0x100 -> 0x101).
    if (cnt_after & crate::interrupt::preempt::PREEMPT_MASK) == 1 {
        slot.otask.store(task as usize, Ordering::Release);
        slot.osite.store(loc as *const Location as usize, Ordering::Release);
        slot.ojif
            .store(crate::drivers::timer::get_jiffies(), Ordering::Release);
        let cpu = crate::arch::cpu_id() as u32;
        slot.ocpu.store(cpu, Ordering::Release);
        // tp-vs-slot divergence at acquire time: the sched slot names
        // another task than tp does. In the steady state these agree;
        // they diverge only inside the pick-publish -> __switch_to
        // window (IRQs off — no acquire can run there) and in the
        // stale-tp / aborted-switch families. A set flag on a leak line
        // is strong stale-booking evidence.
        let mis = if (cpu as usize) < crate::config::MAX_CPUS {
            // Volatile: the sched slot is written by its owning CPU under
            // the GRQ; this is a racy diagnostic read (same discipline as
            // r83hunt's header reads).
            let slot_task = unsafe {
                core::ptr::read_volatile(
                    &crate::sched::sched::cpu_state(cpu as usize).current
                        as *const *mut crate::process::Task
                ) as usize
            };
            if slot_task != task as usize {
                slot.oslot.store(slot_task, Ordering::Release);
                // ph5/ph5b verdict (2/2 boots wedged vs ph3's clean 1200s
                // with this print's absence as the only tree delta): the
                // LIVE print from inside the acquire path is wedged-boot
                // correlated — record-only now. The data still surfaces
                // at adjudication time (oslot= on the ORIGIN lines).
                report_acquire_divergent(task, loc, cpu, slot_task);
                1
            } else {
                0
            }
        } else {
            2
        };
        slot.omis.store(mis, Ordering::Release);
    }
}

/// Live report of a divergent outermost acquire (preempt-hunt). Record-
/// only by default since 14d04456: ph5/ph5b — the only boots carrying
/// this print — both wedged (UNIX_TABLE both-CPU deadlock at 18s/90s)
/// while ph3, without it, completed 1200s cleanly. CONFOUND NOTE (Face C
/// re-trial, 2026-11-02): the ph-session branch was built AFTER 192bfdce
/// (perf-sched merge 11-01 01:27) — every ph boot, wedged or clean, ran
/// with the x86_64 IRQ-exit kpreempt hook armed by default; the wedge-
/// fix lane's bisect (28b1afca) proved hook-ON causes exactly the
/// both-CPU DEADLOCK family. The print was never isolated from that
/// cause. Re-armed via the `dfx=acqdiv` runtime switch for the A/B: the
/// print rides only on boots that ask for it, on a tree where kpreempt
/// defaults OFF.
pub static DIVERGENT_PRINT: AtomicBool = AtomicBool::new(false);

#[allow(unsafe_code)]
fn report_acquire_divergent(
    task: *mut crate::process::Task,
    loc: &'static Location,
    cpu: u32,
    slot_task: usize,
) {
    if !DIVERGENT_PRINT.load(Ordering::Relaxed) {
        let _ = (task, loc, cpu, slot_task);
        return;
    }
    static N: AtomicU32 = AtomicU32::new(60);
    let b = N.load(Ordering::Acquire);
    if b == 0 {
        return;
    }
    if N.compare_exchange(b, b - 1, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return;
    }
    use crate::console::putchar_no_lock as putchar;
    // SAFETY: diagnostic scalar reads of stable Task fields; `task` is
    // current on this CPU (pinned by execution), `slot_task` was this
    // CPU's slot content moments before (same discipline as the s14
    // IRQ-PAIR report).
    unsafe {
        let pid = (*task).pid();
        let pc = (*task).preempt_count();
        let slot_pid = if slot_task >= 0x80000000 {
            (*(slot_task as *mut crate::process::Task)).pid()
        } else {
            0
        };
        for &c in b"\nACQUIRE-DIVERGENT pid=" {
            putchar(c);
        }
        let mut v = pid as u64;
        let mut buf = [0u8; 12];
        let mut n = 0;
        while v > 0 && n < buf.len() {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        if n == 0 {
            putchar(b'0');
        }
        while n > 0 {
            n -= 1;
            putchar(buf[n]);
        }
        for &c in b" pc=0x" {
            putchar(c);
        }
        let mut x = pc as u32 as u64;
        for sh in (0..8).rev() {
            let d = ((x >> (sh * 4)) & 0xF) as u8;
            putchar(if d < 10 { b'0' + d } else { b'a' + d - 10 });
        }
        for &c in b" cpu=" {
            putchar(c);
        }
        let mut v = cpu as u64;
        let mut n = 0;
        while v > 0 && n < buf.len() {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        if n == 0 {
            putchar(b'0');
        }
        while n > 0 {
            n -= 1;
            putchar(buf[n]);
        }
        for &c in b" site=" {
            putchar(c);
        }
        for &c in loc.file().as_bytes() {
            putchar(c);
        }
        putchar(b':');
        let mut v = loc.line() as u64;
        let mut n = 0;
        while v > 0 && n < buf.len() {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        if n == 0 {
            putchar(b'0');
        }
        while n > 0 {
            n -= 1;
            putchar(buf[n]);
        }
        for &c in b" slotpid=" {
            putchar(c);
        }
        let mut v = slot_pid as u64;
        let mut n = 0;
        while v > 0 && n < buf.len() {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        if n == 0 {
            putchar(b'0');
        }
        while n > 0 {
            n -= 1;
            putchar(buf[n]);
        }
        putchar(b'\n');
    }
}

/// Outermost base release (base field 1->0): clears the leak-origin
/// record — that acquire paired. Called from `Task::sub_preempt_count_at`
/// / `sub_preempt_count2_at` when val == PREEMPT_OFFSET and the base
/// field transitions 1->0.
///
/// Pair-split face: if the recorded outermost acquire names a DIFFERENT
/// task than the releaser, the acquire's 1->0 never ran on its own task
/// — either a slot collision (two tasks hashing to one slot; the report
/// carries both pointers so collisions are identifiable) or a genuine
/// cross-task booking split. Budgeted; the flag is evidence, not proof.
#[inline]
pub fn note_release(task: *mut crate::process::Task) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let slot = slot_of(task);
    let ot = slot.otask.load(Ordering::Acquire);
    if ot == 0 {
        return;
    }
    if ot == task as usize {
        slot.otask.store(0, Ordering::Release);
        return;
    }
    // Split: budgeted direct console (may fire from IRQ-laden contexts).
    static SPLIT_N: AtomicU32 = AtomicU32::new(40);
    let b = SPLIT_N.load(Ordering::Acquire);
    if b == 0 {
        return;
    }
    if SPLIT_N
        .compare_exchange(b, b - 1, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    use crate::console::putchar_no_lock as putchar;
    fn ph(v: usize) {
        use crate::console::putchar_no_lock as putchar;
        for sh in (0..16).rev() {
            let n = ((v >> (sh * 4)) & 0xF) as u8;
            putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
        }
    }
    for &c in b"\nACQUIRE-PAIR-SPLIT releaser=0x" {
        putchar(c);
    }
    ph(task as usize);
    for &c in b" recorded=0x" {
        putchar(c);
    }
    ph(ot);
    // The recorded acquire's site + cpu identify where the orphan 0->1
    // came from.
    let site = slot.osite.load(Ordering::Acquire) as *const Location;
    if !site.is_null() {
        // SAFETY: stored from a &'static Location (see note_acquire).
        let loc = unsafe { &*site };
        for &c in b" osite=" {
            putchar(c);
        }
        for &c in loc.file().as_bytes() {
            putchar(c);
        }
        putchar(b':');
        let mut v = loc.line() as u64;
        let mut buf = [0u8; 20];
        let mut n = 0;
        while v > 0 {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        while n > 0 {
            n -= 1;
            putchar(buf[n]);
        }
    }
    for &c in b" ocpu=" {
        putchar(c);
    }
    let mut v = slot.ocpu.load(Ordering::Acquire) as u64;
    let mut buf = [0u8; 20];
    let mut n = 0;
    if v == 0 {
        putchar(b'0');
    }
    while v > 0 {
        buf[n] = b'0' + (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    while n > 0 {
        n -= 1;
        putchar(buf[n]);
    }
    // stress-port r8 discriminators: the RECORDED task's live state at
    // print time. The r7 forker boot saturated this print's 40-line
    // budget with osite=sched.rs:2641 pairs (including symmetric
    // swapped releaser/recorded pairs across cpu0/cpu1) — the shape of
    // two live tasks sharing one 2048-entry hash slot under fork churn,
    // their __schedule GRQ acquire/release windows (~µs) interleaving
    // across CPUs, each release observing the other's still-armed
    // o-record. That cross-talk shows the recorded task OWNED (slot- or
    // SWITCHING_OUT-named) with its base count in flight or already 0;
    // a GENUINE cross-task booking (the lying-slot family) shows the
    // recorded task owned by nothing with base stuck at 1 (its release
    // truly never ran). rpid/rpc/rown let every line self-adjudicate.
    // SAFETY: diagnostic reads of stable Task fields on a pointer that
    // was executing on `ocpu` moments ago; same discipline as
    // note_acquire's sched-slot read.
    if ot >= 0x80000000 {
        unsafe {
            let rt = ot as *mut crate::process::Task;
            for &c in b" rpid=" {
                putchar(c);
            }
            let mut v = (*rt).pid() as u64;
            let mut n = 0;
            if v == 0 {
                putchar(b'0');
            }
            while v > 0 && n < buf.len() {
                buf[n] = b'0' + (v % 10) as u8;
                v /= 10;
                n += 1;
            }
            while n > 0 {
                n -= 1;
                putchar(buf[n]);
            }
            for &c in b" rpc=" {
                putchar(c);
            }
            let mut v = ((*rt).preempt_count()
                & crate::interrupt::preempt::PREEMPT_MASK) as u64;
            let mut n = 0;
            if v == 0 {
                putchar(b'0');
            }
            while v > 0 && n < buf.len() {
                buf[n] = b'0' + (v % 10) as u8;
                v /= 10;
                n += 1;
            }
            while n > 0 {
                n -= 1;
                putchar(buf[n]);
            }
            // rown bit 1: some cpu's sched slot names it; bit 2: some
            // cpu is switching it out right now.
            let mut rown: u64 = 0;
            for c in 0..crate::config::MAX_CPUS {
                if crate::sched::sched::cpu_state(c).current == rt {
                    rown |= 1;
                }
            }
            if crate::sched::sched::task_is_mid_switch_out(rt) {
                rown |= 2;
            }
            for &c in b" rown=" {
                putchar(c);
            }
            let mut v = rown;
            let mut n = 0;
            if v == 0 {
                putchar(b'0');
            }
            while v > 0 && n < buf.len() {
                buf[n] = b'0' + (v % 10) as u8;
                v /= 10;
                n += 1;
            }
            while n > 0 {
                n -= 1;
                putchar(buf[n]);
            }
        }
    }
    putchar(b'\n');
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

/// Record a SOFTIRQ_OFFSET acquire (last wins; phantom-bits extension —
/// the stranded-0x100 monopolist face). `loc` is softirq.rs's
/// `__do_softirq` entry or seqlock.rs's `write` with #[track_caller]
/// propagation, so a stranded 0x100 names its booker directly. Also
/// stamps tp-vs-sched-slot divergence at booking time: a divergent
/// booking is the switch-window family (the release then lands on the
/// task tp names at ITS time, splitting the pair).
#[inline]
pub fn note_softirq_acquire(
    task: *mut crate::process::Task,
    loc: &'static Location,
    cnt_after: i32,
) {
    let slot = slot_of(task);
    slot.task.store(task as usize, Ordering::Release);
    slot.ssite.store(loc as *const Location as usize, Ordering::Release);
    slot.sjif
        .store(crate::drivers::timer::get_jiffies(), Ordering::Release);
    let cpu = crate::arch::cpu_id() as u32;
    slot.scpu.store(cpu, Ordering::Release);
    let mis = if (cpu as usize) < crate::config::MAX_CPUS {
        // Volatile racy diagnostic read (same discipline as note_acquire).
        let slot_task = unsafe {
            core::ptr::read_volatile(
                &crate::sched::sched::cpu_state(cpu as usize).current
                    as *const *mut crate::process::Task
            ) as usize
        };
        if slot_task != task as usize { 1 } else { 0 }
    } else {
        2
    };
    slot.smis.store(mis, Ordering::Release);
}

/// Softirq field drained to zero: the acquire paired — drop the shadow.
#[inline]
pub fn note_softirq_release(task: *mut crate::process::Task) {
    let slot = slot_of(task);
    slot.ssite.store(0, Ordering::Release);
}

/// Hardirq orphan twin (phantom-bits): print the recorded hsite for a
/// task observed carrying a hardirq bit at tick ENTRY (stranded from an
/// earlier bracket — the tick's own booking is not in that snapshot).
pub fn report_hardirq_origin(tag: &[u8], task: *mut crate::process::Task) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let slot = slot_of(task);
    let site = slot.hsite.load(Ordering::Acquire) as *const Location;
    if site.is_null() {
        return;
    }
    static HBUDGET: AtomicU32 = AtomicU32::new(120);
    let b = HBUDGET.load(Ordering::Acquire);
    if b == 0 {
        return;
    }
    if HBUDGET
        .compare_exchange(b, b - 1, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    use crate::console::putchar_no_lock as putchar;
    fn putd(mut v: u64) {
        let mut buf = [0u8; 20];
        let mut n = 0;
        if v == 0 {
            putchar(b'0');
            return;
        }
        while v > 0 && n < buf.len() {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        for i in (0..n).rev() {
            putchar(buf[i]);
        }
    }
    for &c in b"\n" {
        putchar(c);
    }
    for &c in tag {
        putchar(c);
    }
    for &c in b" pid=" {
        putchar(c);
    }
    putd(unsafe { (*task).pid() } as u64);
    // SAFETY: stored from a &'static Location (see note_hardirq_acquire).
    let loc = unsafe { &*site };
    for &c in b" hsite=" {
        putchar(c);
    }
    for &c in loc.file().as_bytes() {
        putchar(c);
    }
    putchar(b':');
    putd(loc.line() as u64);
    for &c in b" hage_jif=" {
        putchar(c);
    }
    putd(
        crate::drivers::timer::get_jiffies()
            .wrapping_sub(slot.hjif.load(Ordering::Acquire)),
    );
    putchar(b'\n');
}

/// Print the SOFTIRQ acquire record for `task` (the phantom-bits tap):
/// names the site that booked a still-set 0x100. Called from the
/// tick-side orphan scan (preempt.rs) and the heal reports. Budgeted
/// separately so orphan lines cannot be starved.
pub fn report_softirq_origin(tag: &[u8], task: *mut crate::process::Task) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let slot = slot_of(task);
    let site = slot.ssite.load(Ordering::Acquire) as *const Location;
    if site.is_null() {
        return; // armed late / wild write (no booking record)
    }
    static SBUDGET: AtomicU32 = AtomicU32::new(240);
    let b = SBUDGET.load(Ordering::Acquire);
    if b == 0 {
        return;
    }
    if SBUDGET
        .compare_exchange(b, b - 1, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    use crate::console::putchar_no_lock as putchar;
    fn putd(mut v: u64) {
        let mut buf = [0u8; 20];
        let mut n = 0;
        if v == 0 {
            putchar(b'0');
            return;
        }
        while v > 0 && n < buf.len() {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        for i in (0..n).rev() {
            putchar(buf[i]);
        }
    }
    for &c in b"\n" {
        putchar(c);
    }
    for &c in tag {
        putchar(c);
    }
    for &c in b" pid=" {
        putchar(c);
    }
    putd(unsafe { (*task).pid() } as u64);
    for &c in b" pc=0x" {
        putchar(c);
    }
    let pc = unsafe { (*task).preempt_count() } as u32 as u64;
    for sh in (0..8).rev() {
        let n = ((pc >> (sh * 4)) & 0xF) as u8;
        putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
    }
    // SAFETY: stored from a &'static Location (see note_softirq_acquire).
    let loc = unsafe { &*site };
    for &c in b" ssite=" {
        putchar(c);
    }
    for &c in loc.file().as_bytes() {
        putchar(c);
    }
    putchar(b':');
    putd(loc.line() as u64);
    for &c in b" scpu=" {
        putchar(c);
    }
    putd(slot.scpu.load(Ordering::Acquire) as u64);
    if slot.smis.load(Ordering::Acquire) != 0 {
        for &c in b" smis=1" {
            putchar(c);
        }
    }
    for &c in b" sage_jif=" {
        putchar(c);
    }
    putd(
        crate::drivers::timer::get_jiffies()
            .wrapping_sub(slot.sjif.load(Ordering::Acquire)),
    );
    putchar(b'\n');
}

/// User-mode return tripwire. Call at the trap-handler tail when the
/// frame returns to userspace: preempt_count MUST be 0 there. Direct
/// console, latched per episode, budgeted.
///
/// preempt-hunt rate gate: a task carrying a stuck +1 re-acquires inner
/// guards constantly (every lock it touches re-arms the episode latch),
/// which flooded the shared budget within seconds (374 lines for one
/// carrier in the first 20s of ph1). Space reports 50 jiffies apart per
/// task and give the face its own budget.
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
    let now = crate::drivers::timer::get_jiffies();
    let last = slot.urrep_jif.load(Ordering::Acquire);
    if now.wrapping_sub(last) < 50 {
        return;
    }
    slot.urrep_jif.store(now, Ordering::Release);
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

/// Print the OUTERMOST-acquire record for `task` (the preempt-hunt tap):
/// names the site whose 1->0 release never ran. Called from the
/// WAKE-HEAL zeroing (sched.rs, GRQ held — direct console only) and the
/// underflow clamp report: the two points where a nonzero base count is
/// adjudicated. Budgeted separately from BUDGET (the origin line is the
/// hunt's primary output; it must not be starved by parked-adjacent
/// noise).
pub fn report_outermost(tag: &[u8], task: *mut crate::process::Task) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let slot = slot_of(task);
    if slot.otask.load(Ordering::Acquire) != task as usize {
        return; // no outermost record for this task (armed late / healed)
    }
    static OBUDGET: AtomicU32 = AtomicU32::new(200);
    let b = OBUDGET.load(Ordering::Acquire);
    if b == 0 {
        return;
    }
    if OBUDGET
        .compare_exchange(b, b - 1, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    use crate::console::putchar_no_lock as putchar;
    fn putd(mut v: u64) {
        use crate::console::putchar_no_lock as putchar;
        let mut buf = [0u8; 20];
        let mut n = 0;
        if v == 0 {
            putchar(b'0');
            return;
        }
        while v > 0 && n < buf.len() {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        for i in (0..n).rev() {
            putchar(buf[i]);
        }
    }
    for &c in b"\n" {
        putchar(c);
    }
    for &c in tag {
        putchar(c);
    }
    for &c in b" pid=" {
        putchar(c);
    }
    putd(unsafe { (*task).pid() } as u64);
    let site = slot.osite.load(Ordering::Acquire) as *const Location;
    if !site.is_null() {
        // SAFETY: stored from a &'static Location (see note_acquire).
        let loc = unsafe { &*site };
        for &c in b" osite=" {
            putchar(c);
        }
        for &c in loc.file().as_bytes() {
            putchar(c);
        }
        putchar(b':');
        putd(loc.line() as u64);
    } else {
        for &c in b" osite=?" {
            putchar(c);
        }
    }
    for &c in b" ocpu=" {
        putchar(c);
    }
    putd(slot.ocpu.load(Ordering::Acquire) as u64);
    let mis = slot.omis.load(Ordering::Acquire);
    if mis != 0 {
        for &c in b" omis=" {
            putchar(c);
        }
        putd(mis as u64);
        let osl = slot.oslot.load(Ordering::Acquire);
        if osl != 0 {
            for &c in b" oslot=0x" {
                putchar(c);
            }
            for sh in (0..16).rev() {
                let n = ((osl >> (sh * 4)) & 0xF) as u8;
                putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
            }
        }
    }
    for &c in b" oage_jif=" {
        putchar(c);
    }
    putd(
        crate::drivers::timer::get_jiffies()
            .wrapping_sub(slot.ojif.load(Ordering::Acquire)),
    );
    putchar(b'\n');
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
    // phantom-bits: the SOFTIRQ acquire shadow — names which +0x100
    // booker (__do_softirq bracket vs SeqLock::write) owns a stranded
    // softirq bit on this task, and when.
    let ssite = slot.ssite.load(Ordering::Acquire) as *const Location;
    if slot.task.load(Ordering::Acquire) == task as usize && !ssite.is_null() {
        // SAFETY: stored from a &'static Location (see note_softirq_acquire).
        let loc = unsafe { &*ssite };
        for &c in b" ssite=" {
            putchar(c);
        }
        for &c in loc.file().as_bytes() {
            putchar(c);
        }
        putchar(b':');
        put_i64(loc.line() as i64);
        for &c in b" sage_jif=" {
            putchar(c);
        }
        put_i64(now.wrapping_sub(slot.sjif.load(Ordering::Acquire)) as i64);
    }
    for &c in b" cnt_after=" {
        putchar(c);
    }
    put_i64(slot.cnt_after.load(Ordering::Acquire) as i64);
    for &c in b" age_jif=" {
        putchar(c);
    }
    put_i64(now.wrapping_sub(slot.jif.load(Ordering::Acquire)) as i64);
    // Outermost-acquire tap: when the reported task is the one holding
    // the unreturned 0->1, its site is the leak's origin (stable across
    // inner 1->2->1 pairs; see LoSlot.otask).
    if slot.otask.load(Ordering::Acquire) == task as usize {
        let osite = slot.osite.load(Ordering::Acquire) as *const Location;
        if !osite.is_null() {
            // SAFETY: stored from a &'static Location (see note_acquire).
            let loc = unsafe { &*osite };
            for &c in b" osite=" {
                putchar(c);
            }
            for &c in loc.file().as_bytes() {
                putchar(c);
            }
            putchar(b':');
            put_i64(loc.line() as i64);
        }
        for &c in b" ocpu=" {
            putchar(c);
        }
        put_i64(slot.ocpu.load(Ordering::Acquire) as i64);
        if slot.omis.load(Ordering::Acquire) != 0 {
            for &c in b" omis=" {
                putchar(c);
            }
            put_i64(slot.omis.load(Ordering::Acquire) as i64);
            let osl = slot.oslot.load(Ordering::Acquire);
            if osl != 0 {
                for &c in b" oslot=0x" {
                    putchar(c);
                }
                for sh in (0..16).rev() {
                    let n = ((osl >> (sh * 4)) & 0xF) as u8;
                    putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                }
            }
        }
    }
    putchar(b'\n');
}
