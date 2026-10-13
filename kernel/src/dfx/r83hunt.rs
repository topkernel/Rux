//! R83 (R74 slotless-double-run producer hunt): live task-header audit.
//!
//! The R74-SLOTLESS-TP / PREEMPT-UNDERFLOW family dies with a thread_info
//! header that was overwritten by FOREIGN DATA: every capture shows
//! ti_kernel_sp holding a bit63-CLEAR qword whose halves look like
//! kernel-heap payload (decimal ksp prints 429496725|461472768 and
//! 429496725|486114304 — same upper half 0x1999_9995, 1 KiB-aligned lower
//! halves; bigalloc9: 94967254|80609280), while the SAME task also reads
//! preempt_count == -1. The garbage is written ONCE and stays frozen
//! (bigalloc9 printed the identical ksp on four consecutive R74 lines).
//! None of the in-kernel writers (trap.S exit paths, exec pin, fork init,
//! free clear) can produce such a value — so a wild write or a recycled
//! page (Task freed while still referenced) is the producer.
//!
//! This module makes the corruption visible the MOMENT it exists instead
//! of at the next schedule of the victim:
//!
//! - `audit_tick()` (idle-loop hook, rate-limited to one system-wide scan
//!   every 4 jiffies) walks the pid hash and validates the words every
//!   producer must leave plausible: ti_kernel_sp (0 or kernel-half and
//!   not inside the kernel text), ti_preempt_count (0..=1024, never
//!   negative), ti_pick_state (<= PICK_REVOKED), running_on_cpu
//!   (-1..MAX_CPUS), and the state word (never TASK_POISON while hashed).
//! - `dump_victim()` prints the raw first 0x40 header bytes at the R74
//!   and PREEMPT-UNDERFLOW detection sites, so the byte-level shape of
//!   the scribble (which fields, which alignment) is captured even when
//!   the audit missed the write window.
//! - free_task_slot paints the freed page header with R83_FREED_PAINT
//!   (sched.rs): a victim whose header reads the paint proves the
//!   use-after-free shape outright.
//!
//! Reporting discipline: raw UART only (callers may hold bucket locks /
//! run with IRQs off), one report per distinct victim address (8-entry
//! ring), hard cap of 16 printed reports so the console cannot flood.

use crate::console::putchar_no_lock as putchar;
use crate::process::task::{task_offsets as offs, Task};
use crate::sched::sched::TASK_POISON;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Paint written over a freed Task page header (sched.rs free path).
pub const R83_FREED_PAINT: u64 = 0xC3C3_C3C3_C3C3_C3C3;

/// Up to four implausible (field, value) pairs per victim.
pub type Fields = [(&'static [u8], u64); 4];
const EMPTY: (&'static [u8], u64) = (b"", 0);
pub const NO_FIELDS: Fields = [EMPTY, EMPTY, EMPTY, EMPTY];

/// Distinct-victim dedup ring (page addresses).
static SEEN: [AtomicU64; 8] = [const { AtomicU64::new(0) }; 8];
/// Total printed reports; past the cap the hunt goes quiet (count-only).
static REPORTS: AtomicU32 = AtomicU32::new(0);
/// Total detections (printed or suppressed) — pulse for taskdump.
pub static DETECTED: AtomicU32 = AtomicU32::new(0);

fn phex(mut v: u64) {
    for _ in 0..16 {
        let n = (v >> 60) as u8;
        putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
        v <<= 4;
    }
}

fn pdec(mut v: u64) {
    let mut buf = [0u8; 10];
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

fn pmsg(m: &[u8]) {
    for &b in m {
        putchar(b);
    }
}

/// One report per distinct victim page, capped globally at 16 prints.
fn may_report(addr: u64) -> bool {
    if REPORTS.load(Ordering::Relaxed) >= 16 {
        return false;
    }
    for s in SEEN.iter() {
        let cur = s.load(Ordering::Relaxed);
        if cur == addr {
            return false;
        }
        if cur == 0
            && s.compare_exchange(0, addr, Ordering::Relaxed, Ordering::Relaxed).is_ok()
        {
            return true;
        }
    }
    false
}

/// Raw volatile reads of the Task header (diagnostic only).
#[inline]
unsafe fn rq64(task: *mut Task, off: usize) -> u64 {
    core::ptr::read_volatile((task as usize + off) as *const u64)
}
#[inline]
unsafe fn rq32(task: *mut Task, off: usize) -> u32 {
    core::ptr::read_volatile((task as usize + off) as *const u32)
}

/// The single reporting path: gates on the dedup ring, prints the victim
/// identity, the implausible fields, and the first 0x40 header bytes.
fn report(tag: &[u8], task: *mut Task, bad: &Fields) {
    if task.is_null() || (task as usize) < 0x8000_0000_0000_0000 {
        return;
    }
    DETECTED.fetch_add(1, Ordering::Relaxed);
    if !may_report(task as u64) {
        return;
    }
    REPORTS.fetch_add(1, Ordering::Relaxed);
    pmsg(b"\nR83-VICTIM tag=");
    pmsg(tag);
    pmsg(b" jiffies=");
    pdec(crate::drivers::timer::get_jiffies());
    pmsg(b" by=cpu:");
    pdec(crate::arch::cpu_id() as u64);
    pmsg(b"/pid:");
    pdec(crate::sched::get_current_pid() as u64);
    pmsg(b" task=0x");
    phex(task as u64);
    pmsg(b" pid=");
    unsafe { pdec(rq32(task, offs::TASK_PID) as u64); }
    pmsg(b" comm=");
    unsafe {
        for &c in (*task).comm().iter() {
            if c == 0 {
                break;
            }
            putchar(if (0x20..0x7f).contains(&c) { c } else { b'.' });
        }
    }
    pmsg(b" kstack=0x");
    unsafe {
        match (*task).get_kernel_stack() {
            Some(bottom) => {
                phex(bottom as u64);
                pmsg(b"-0x");
                phex(bottom as u64 + crate::config::KERNEL_STACK_SIZE as u64);
            }
            None => pmsg(b"none"),
        }
    }
    for (name, v) in bad.iter() {
        if name.is_empty() {
            break;
        }
        pmsg(b" ");
        pmsg(name);
        pmsg(b"=0x");
        phex(*v);
    }
    putchar(b'\n');
    // Header hex: 8 qwords with byte offsets — the byte-level shape of
    // the scribble (extent + alignment) identifies the writer's store.
    for row in 0..2 {
        pmsg(b"R83-HDR ");
        pdec((row * 0x20) as u64);
        pmsg(b":");
        for col in 0..4 {
            putchar(b' ');
            unsafe { phex(rq64(task, row * 0x20 + col * 8)); }
        }
        putchar(b'\n');
    }
}

/// Print the header of an already-suspect task (R74 / PREEMPT-UNDERFLOW
/// detection sites). No field validation — the caller already knows.
pub fn dump_victim(tag: &[u8], task: *mut Task) {
    report(tag, task, &NO_FIELDS);
}

fn text_range() -> (u64, u64) {
    // _stext/_etext: same linker symbols the R74 sp-gate compares against.
    extern "C" {
        static _stext: u8;
        static _etext: u8;
    }
    unsafe { (&_stext as *const u8 as u64, &_etext as *const u8 as u64) }
}

/// Field validation: returns up to four implausible (name, value) pairs.
/// Rules are deliberately conservative — a false positive here would bury
/// the real producer line in noise:
/// - ti_kernel_sp: 0 (fresh/idle) or kernel-half AND outside [_stext,
///   _etext). The captured garbage is bit63-CLEAR user-range data.
/// - ti_preempt_count: only NEGATIVE values are implausible (the
///   PREEMPT-UNDERFLOW shape). Large positives are legal: a task on
///   another CPU may legitimately sit mid-hardirq (HARDIRQ_OFFSET =
///   0x1_0000, plus PREEMPT_OFFSET per held spinlock) at the moment the
///   audit walks it — the r1 hunt run produced 8 false hits flagging
///   0x1_0001/0x1_0000 before this was narrowed.
/// - ti_pick_state: PICKED/LIVE/REVOKED only.
/// - running_on_cpu: -1 or a valid CPU id.
/// - state word: never TASK_POISON while still hashed (freed-while-
///   linked), and no freed-paint qword in the header (recycled page).
unsafe fn bad_fields(task: *mut Task) -> Fields {
    let mut bad = NO_FIELDS;
    let mut n = 0usize;
    macro_rules! push {
        ($name:expr, $v:expr) => {
            if n < 4 {
                bad[n] = ($name, $v as u64);
                n += 1;
            }
        };
    }
    for i in 0..8 {
        let v = rq64(task, i * 8);
        if v == R83_FREED_PAINT {
            push!(b"freed-paint", v);
            break;
        }
    }
    if rq32(task, offs::TASK_STATE) == TASK_POISON {
        push!(b"state=POISON", TASK_POISON as u64);
    }
    let ksp = rq64(task, offs::TI_KERNEL_SP);
    if ksp != 0 {
        if (ksp as i64) >= 0 {
            push!(b"ti_kernel_sp(user-half)", ksp);
        } else {
            let (lo, hi) = text_range();
            if ksp >= lo && ksp < hi {
                push!(b"ti_kernel_sp(text)", ksp);
            }
        }
    }
    let pc = rq32(task, offs::TI_PREEMPT_COUNT) as i32;
    if pc < 0 {
        push!(b"ti_preempt_count", pc as u32 as u64);
    }
    let pk = rq32(task, offs::TI_PICK_STATE);
    if pk > 2 {
        push!(b"ti_pick_state", pk as u64);
    }
    let claim = rq32(task, offs::TI_RUNNING_ON_CPU) as i32;
    if claim < -1 || claim >= crate::config::MAX_CPUS as i32 {
        push!(b"running_on_cpu", claim as u32 as u64);
    }
    bad
}

/// Idle-loop hook: one system-wide scan every 4 jiffies (CAS-gated so a
/// single CPU runs it). Each pass walks the pid hash try-locked and
/// reports the first implausible header it sees.
pub fn audit_tick() {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = crate::drivers::timer::get_jiffies();
    let last = LAST.load(Ordering::Acquire);
    if now.wrapping_sub(last) < 4 {
        return;
    }
    if LAST
        .compare_exchange(last, now, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    crate::process::pid_hash::pid_hash_for_each_task_try(|task| {
        let pid = unsafe { (*task).pid() };
        if pid == 0 || pid == TASK_POISON {
            return;
        }
        let bad = unsafe { bad_fields(task) };
        if !bad[0].0.is_empty() {
            report(b"audit", task, &bad);
        }
    });
}

// ==================== R83b: ext4-biglock waiter self-watch ====================

/// Header snapshot for the waiter self-watch: the exact words a foreign
/// store has been observed to hit (ti_kernel_sp) plus their neighbors.
#[derive(Copy, Clone, PartialEq)]
pub struct WatchSnap {
    pub ksp: u64,
    pub user_sp: u64,
    pub preempt: u32,
    pub pick: u32,
    pub claim: i32,
    pub flags: u32,
}

pub fn watch_snap(task: *mut Task) -> WatchSnap {
    unsafe {
        WatchSnap {
            ksp: rq64(task, offs::TI_KERNEL_SP),
            user_sp: rq64(task, offs::TI_USER_SP),
            preempt: rq32(task, offs::TI_PREEMPT_COUNT),
            pick: rq32(task, offs::TI_PICK_STATE),
            claim: rq32(task, offs::TI_RUNNING_ON_CPU) as i32,
            flags: rq32(task, offs::TI_FLAGS),
        }
    }
}

/// A blocked task's header changed while it was parked in the EXT4
/// big-lock wait — the R74 producer's write window (every R74 capture so
/// far: the victim sat in this wait when its ti_kernel_sp turned to
/// foreign data). Reports the before/after words, the full header, and
/// the memory AROUND the struct (a neighboring-buffer overrun shows as a
/// run of similar values). Raw UART only; the caller must hold no locks.
pub fn waitwatch_report(task: *mut Task, old: &WatchSnap, new: &WatchSnap) {
    DETECTED.fetch_add(1, Ordering::Relaxed);
    if !may_report(task as u64) {
        return;
    }
    REPORTS.fetch_add(1, Ordering::Relaxed);
    pmsg(b"\nR83-WAITWATCH jiffies=");
    pdec(crate::drivers::timer::get_jiffies());
    pmsg(b" task=0x");
    phex(task as u64);
    pmsg(b" pid=");
    unsafe { pdec(rq32(task, offs::TASK_PID) as u64); }
    pmsg(b"\n  old: ksp=0x");
    phex(old.ksp);
    pmsg(b" user_sp=0x");
    phex(old.user_sp);
    pmsg(b" preempt=");
    pdec(old.preempt as u64);
    pmsg(b" pick=");
    pdec(old.pick as u64);
    pmsg(b" claim=");
    pdec(old.claim as i64 as u64);
    pmsg(b"\n  new: ksp=0x");
    phex(new.ksp);
    pmsg(b" user_sp=0x");
    phex(new.user_sp);
    pmsg(b" preempt=");
    pdec(new.preempt as u64);
    pmsg(b" pick=");
    pdec(new.pick as u64);
    pmsg(b" claim=");
    pdec(new.claim as i64 as u64);
    putchar(b'\n');
    // Full header (0..0x40) + the eight qwords after it, so an
    // overrun's extent shows. (No pre-struct reads: Task pages are
    // page-aligned and ti_flags sits at offset 0 — reading backwards
    // would cross into the previous page; the r3 hunt run panicked in
    // this printer on the wrapped offset before that was caught.)
    for row in 0..2 {
        pmsg(b"R83-HDR ");
        pdec((row * 0x20) as u64);
        pmsg(b":");
        for col in 0..4 {
            putchar(b' ');
            unsafe { phex(rq64(task, row * 0x20 + col * 8)); }
        }
        putchar(b'\n');
    }
    pmsg(b"R83-AROUND +0x40:");
    for i in 0..8 {
        putchar(b' ');
        unsafe { phex(rq64(task, 0x40 + i * 8)); }
    }
    putchar(b'\n');
}
