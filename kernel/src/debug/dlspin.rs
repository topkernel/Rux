//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Spinlock-deadlock last-gasp reporting (debug-dlspin lane).
//!
//! The spinlock slow path (`sync/spinlock.rs`) has always warned after
//! DEADLOCK_WARN_ITERS spin iterations, but the one-line warn named only
//! the WAITER (cpu, lock address, raw ra). A wedge was solvable from the
//! log alone only by luck. This module upgrades that cold path into a
//! LAST-GASP BUNDLE that names both sides:
//!
//! 1. the waiter — cpu, lock address, call site SYMBOLIZED inline;
//! 2. the holder — from the lock's owner word (cpu + acquire call site,
//!    symbolized) when owner recording is armed;
//! 3. a CROSS-CPU PEEK at the holder CPU's current task: comm, pid,
//!    state, preempt count and a bounded kernel-stack walk. This is the
//!    ONLY observer that can see a holder frozen with IF=0 and its own
//!    tick dead — every same-CPU watchdog is blind there (the
//!    allocator-freeze lesson, 2026-11-02);
//! 4. all-CPU stacks (a lockless twin of sysrq-'l' — the sysrq twin
//!    prints through printk, which takes locks the wedge may hold) and
//!    the full `dump_all_tasks` snapshot;
//! 5. `spinpanic=1` turns detection into a panic (the reference
//!    kernel's softlockup_panic/hung_task_panic shape), carrying the
//!    bundle into the pstore console record.
//!
//! Everything prints through the `putchar_no_lock` family and touches
//! no locks and no allocator (the wedged lock may BE the printk, heap
//! or pid-hash lock). The bundle arms the pstore sink first so every
//! byte it prints is staged for the persistent 128 KiB console record.
//!
//! Boot parameters (standalone `key=1` params, oops=/forkprof= style —
//! the bespoke `debug=` switch table is frozen and does not grow):
//!
//! - `spinowner=1` — arm spinlock owner recording. Recording exists
//!   only in `debug-lock-owner` FEATURE builds (the default build's
//!   lock paths carry zero extra instructions — merge-gate mandate);
//!   the flag is the boot-time gate inside such builds. forkprof=1/2
//!   implies it (the GRQ stall probe's holder attribution).
//! - `spinpanic=1` — panic (into pstore) on the first detected
//!   spinlock deadlock. Default off, like the reference's
//!   softlockup_panic.
//! - `spindeadlock=1` — drive a SYNTHETIC 2-CPU deadlock after SMP
//!   comes up: a holder kthread freezes holding the test lock with
//!   interrupts disabled, the driving task spins on it, and the
//!   last-gasp bundle must name both from the log alone. The holder
//!   releases once the bundle completed (bounded by a 30 s safety
//!   cap), so the boot continues afterwards.
//!
//! Bundle policy: the one-line DEADLOCK warn repeats at every
//! DEADLOCK_WARN_ITERS boundary (the spinner keeps spinning — the
//! contention might resolve); the FULL bundle runs ONCE per lock
//! address per boot (a small lock-free registry), because the bundle
//! is thousands of bytes and a wedged machine cannot afford it twice.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// ============================================================================
// Boot flags
// ============================================================================

/// `spinpanic=1`: detection escalates to panic (into pstore).
static SPINPANIC: AtomicBool = AtomicBool::new(false);

/// `spindeadlock=1`: drive the synthetic deadlock after SMP comes up.
static SPINDEADLOCK_TEST: AtomicBool = AtomicBool::new(false);

/// `spinowner=1` (or forkprof armed): spinlock owner recording on.
/// Only exists in `debug-lock-owner` feature builds; see
/// `spin::spinlock::RawSpinlock::record_owner` for the cost contract.
#[cfg(feature = "debug-lock-owner")]
static SPINOWNER_ARMED: AtomicBool = AtomicBool::new(false);

/// Whether spinlock owner recording is armed in THIS boot.
#[inline]
pub fn spinowner_armed() -> bool {
    #[cfg(feature = "debug-lock-owner")]
    {
        SPINOWNER_ARMED.load(Ordering::Relaxed)
    }
    #[cfg(not(feature = "debug-lock-owner"))]
    {
        false
    }
}

/// Whether the kernel was BUILT with owner recording at all (the
/// debug-lock-owner feature). Report text picks the right arming hint.
#[inline]
pub fn owner_recording_built() -> bool {
    cfg!(feature = "debug-lock-owner")
}

/// `spinpanic` live value.
#[inline]
pub fn spinpanic_on() -> bool {
    SPINPANIC.load(Ordering::Acquire)
}

/// One `key=1|on|true` cmdline flag.
fn flag_on(key: &str) -> bool {
    matches!(
        crate::cmdline::get_param(key).as_deref(),
        Some("1") | Some("on") | Some("true")
    )
}

/// Parse the dlspin boot parameters (debug init). Standalone params on
/// the oops=/forkprof= pattern — the `debug=` switch table is frozen
/// (AGENTS.md observability policy) and these are boot policy flags,
/// not hunt instruments.
pub fn init_from_cmdline() {
    if flag_on("spinpanic") {
        SPINPANIC.store(true, Ordering::Release);
        crate::pr_info!("debug: spinlock deadlock detection will panic (spinpanic)");
    }
    if flag_on("spindeadlock") {
        SPINDEADLOCK_TEST.store(true, Ordering::Release);
    }
    if flag_on("spinowner") {
        #[cfg(feature = "debug-lock-owner")]
        {
            SPINOWNER_ARMED.store(true, Ordering::Release);
            crate::pr_info!("debug: spinlock owner recording armed (spinowner)");
        }
        #[cfg(not(feature = "debug-lock-owner"))]
        {
            crate::pr_warn!(
                "debug: spinowner=1 ignored — kernel built without the debug-lock-owner feature"
            );
        }
    }
    // forkprof hunts read holder call sites through the GRQ stall probe
    // (sched.rs r12); arming owner recording for those boots keeps that
    // probe's holder attribution working without a second cmdline token.
    #[cfg(feature = "debug-lock-owner")]
    if !spinowner_armed() && crate::debug::forkprof::on() {
        SPINOWNER_ARMED.store(true, Ordering::Release);
        crate::pr_info!("debug: spinlock owner recording armed (forkprof)");
    }
}

// ============================================================================
// Lock-free, allocation-free console writers
// ============================================================================

fn putc(b: u8) {
    // putchar_no_lock is the SBI/UART raw write — no locks, no printk,
    // IRQ-off safe (same writer family taskdump uses).
    crate::console::putchar_no_lock(b);
}

fn put_lit(s: &[u8]) {
    // Batched TX (one bounded THRE wait per burst): ~16x less UART
    // register traffic than per-byte putc, and far smaller interleave
    // windows when another CPU chats raw-serial at the same time
    // (same lock-free writer family).
    crate::console::puts_bytes_no_lock(s);
}

fn put_hex(v: u64) {
    let mut shift = 64;
    while shift > 0 {
        shift -= 4;
        let nibble = (v >> shift) & 0xF;
        putc(if nibble < 10 {
            b'0' + nibble as u8
        } else {
            b'a' + (nibble - 10) as u8
        });
    }
}

fn put_dec(mut v: u64) {
    let mut buf = [0u8; 20];
    let mut n = 0;
    loop {
        buf[n] = b'0' + (v % 10) as u8;
        n += 1;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    while n > 0 {
        n -= 1;
        putc(buf[n]);
    }
}

fn put_dec_i(v: i64) {
    if v < 0 {
        putc(b'-');
        put_dec(v.unsigned_abs());
    } else {
        put_dec(v as u64);
    }
}

/// Print `name+0xoff/0xsize` for a code address WITHOUT allocating
/// (`ksyms::format_symbol` returns a String — the heap may be the
/// wedged subsystem). `?` when the table cannot resolve it.
fn put_sym(pc: u64) {
    match crate::debug::ksyms::lookup(pc) {
        Some((name, addr, size)) => {
            put_lit(name.as_bytes());
            putc(b'+');
            put_hex(pc - addr);
            if size > 0 {
                putc(b'/');
                put_hex(size);
            }
        }
        None => put_lit(b"?"),
    }
}

/// A task's comm, printable bytes only (taskdump's filter).
fn put_comm(t: &crate::process::task::Task) {
    let name = t.comm().split(|&b| b == 0).next().unwrap_or(b"?");
    for &b in name.iter().take(16) {
        putc(if b >= 0x20 && b < 0x7f { b } else { b'.' });
    }
}

// ============================================================================
// Stack scan (bounded, symbolized, no frame-pointer walk)
// ============================================================================

/// Stack-word filter: the word must resolve into a SIZED text symbol.
/// The linker types whole-region markers (`__boot_stack_bottom`,
/// `__srodata`, `boot_gdt`, the trailing `_etext` span) as text with
/// size 0 — `lookup` then accepts any garbage word that falls in their
/// (open-ended) span as a "return address" (observed in the 2026-11-08
/// make-test and KVM-proof bundles: canaries and stack pointers
/// printing as `__boot_stack_bottom+...`). Every real function in the
/// table carries a size; requiring one keeps real frames (including
/// underscore-named asm like `__switch_to`) and drops every marker
/// span. Marker names are also excluded by the same rule.
#[inline]
fn kstack_hit(v: u64) -> bool {
    match crate::debug::ksyms::lookup(v) {
        Some((_name, _addr, size)) => size > 0,
        None => false,
    }
}

/// Print up to `max_frames` kernel-text words found scanning
/// `max_scan` words UP from a task's saved kernel sp; when that finds
/// nothing (a task FROZEN ON CPU — it never switched out, so
/// `thread.sp` still points at its switch-in/creation scratch frame at
/// the stack top and every live frame sits BELOW it), fall back to a
/// downward scan capped at one kernel stack, printing the hits
/// NEAREST the saved sp first (the live chain — see the down-scan
/// comment for why deepest-first is wrong there).
///
/// Approximation (same contract as taskdump/sysrq-'l' stack words): the
/// frame-pointer chain of a REMOTE task cannot be walked without its
/// registers, so the scan filters stack words that fall inside a known
/// text symbol (`ksyms::lookup` doubles as the filter — the make-test
/// lesson: hits resolving into the trailing `_etext` span are DATA
/// addresses past text end, not return addresses, and are excluded the
/// same way taskdump's R38 uses a text window). For a holder
/// frozen mid-critical-section the deepest hits are the live chain;
/// older stale frames above them may mix in.
fn put_kstack(sp: u64, max_frames: usize, max_scan: usize) {
    if (sp >> 48) != 0xffff || (sp & 7) != 0 || sp <= 0x1000 {
        put_lit(b" (no kernel sp)\n");
        return;
    }
    let base = sp as *const u64;
    let mut shown = 0;
    put_lit(b" kstack:");
    for k in 0..max_scan {
        // SAFETY: diagnostic read of the task's own kernel-stack pages
        // (mapped while the Task exists); bounded scan above sp, the
        // same read family taskdump's per-CPU bt uses.
        let v = unsafe { core::ptr::read_volatile(base.add(k)) };
        if kstack_hit(v) {
            putc(b'\n');
            put_lit(b"    ");
            put_hex(v);
            putc(b' ');
            put_sym(v);
            shown += 1;
            if shown >= max_frames {
                break;
            }
        }
    }
    if shown > 0 {
        putc(b'\n');
        return;
    }
    // Frozen-on-CPU shape: scan DOWN from the saved sp and print the
    // FIRST `max_frames` symbolized words BELOW it, nearest-first. A
    // task frozen on CPU never switched out, so `thread.sp` still
    // points at its switch-in/creation scratch frame at the stack top
    // and the LIVE chain (creation stub -> kthread fn -> freeze loop)
    // sits in the first words below it. Farther down lies older stack
    // debris from whatever previously used these pages (the 2026-11-10
    // riscv ktest lesson: keeping the DEEPEST hits printed a fresh
    // kthread's stale ext4/sysctl remnants and lost the freeze loop);
    // nearest-first is the selection that names what the CPU is doing
    // NOW. A task that switched out normally never reaches here.
    put_lit(b" (none above sp; below sp, nearest first:)");
    let total = crate::config::KERNEL_STACK_SIZE / 8;
    let mut shown = 0usize;
    for k in 1..=total {
        if shown >= max_frames {
            break;
        }
        // SAFETY: diagnostic read bounded to the kernel-stack region
        // below the task's sp (the stack is KERNEL_STACK_SIZE and
        // mapped while the Task exists); addresses only, no deref of
        // the collected words.
        let p = (base.wrapping_sub(k)) as *const u64;
        if (p as u64) <= 0x1000 {
            break;
        }
        let v = unsafe { core::ptr::read_volatile(p) };
        if kstack_hit(v) {
            putc(b'\n');
            put_lit(b"    ");
            put_hex(v);
            putc(b' ');
            put_sym(v);
            shown += 1;
        }
    }
    if shown == 0 {
        put_lit(b" (no symbolized frames)");
    }
    putc(b'\n');
}

// ============================================================================
// The last-gasp bundle
// ============================================================================

/// Lock addresses that already produced a full bundle this boot.
/// Lock-free registry; 16 distinct deadlocked spinlocks per boot is far
/// past the point where volume matters.
static BUNDLED: [AtomicUsize; 16] = [const { AtomicUsize::new(0) }; 16];

/// Set once the full bundle has completed (the synthetic-deadlock
/// holder releases on it; nothing else reads it).
static LAST_GASP_DONE: AtomicBool = AtomicBool::new(false);

/// Warn count (the synthetic test's "it fired" check).
static WARN_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Claim-or-find: `true` when this lock gets its first bundle.
fn bundle_first_time(lock_addr: usize) -> bool {
    for slot in &BUNDLED {
        match slot.compare_exchange(0, lock_addr, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return true,
            Err(v) if v == lock_addr => return false,
            Err(_) => continue,
        }
    }
    // Registry full — fail open: a 17th distinct wedge deserves the
    // bundle more than the registry deserves its budget.
    true
}

/// The deadlock watchdog entry, called from `RawSpinlock::lock`'s spin
/// loop at every DEADLOCK_WARN_ITERS boundary. Runs with the caller's
/// IRQs (possibly off) and preempt disabled; takes no locks and
/// allocates nothing.
///
/// `owner_word` is the lock's debug-lock-owner field when built (ra |
/// cpu+1; 0 = free/mid-handoff) or 0 when not built.
pub fn deadlock_warn(cpu: usize, lock_addr: usize, waiter_ra: usize, owner_word: usize) {
    WARN_COUNT.fetch_add(1, Ordering::Relaxed);

    // One-line warn EVERY boundary (scanner-compatible with the
    // historical format; the symbolized call site is appended).
    put_lit(b"DEADLOCK: spinlock stuck cpu=");
    put_dec(cpu as u64);
    put_lit(b" lock=0x");
    put_hex(lock_addr as u64);
    put_lit(b" ra=0x");
    put_hex(waiter_ra as u64);
    putc(b' ');
    put_sym(waiter_ra as u64);

    if !bundle_first_time(lock_addr) {
        put_lit(b" [repeat]\n");
        return;
    }
    putc(b'\n');

    // Arm the pstore console sink FIRST: every byte the bundle prints
    // is then staged for the persistent 128 KiB record (and a
    // spinpanic panic captures it wholesale).
    crate::debug::pstore::arm_sink();
    crate::debug::taint::add_taint(crate::debug::taint::TaintFlags::SOFTLOCKUP);

    // --- (b) holder identity from the owner word ---
    let holder_cpu: Option<usize> = if owner_word & 0xF != 0 {
        Some((owner_word & 0xF) as usize - 1)
    } else {
        None
    };
    match holder_cpu {
        Some(h) => {
            put_lit(b"  holder=cpu=");
            put_dec(h as u64);
            put_lit(b" ra=0x");
            put_hex((owner_word & !0xF) as u64);
            putc(b' ');
            put_sym((owner_word & !0xF) as u64);
            putc(b'\n');
            // --- (c) cross-CPU peek at the frozen holder ---
            holder_peek(h);
        }
        None => {
            if spinowner_armed() {
                put_lit(b"  holder=unrecorded (owner word 0: lock mid-handoff)\n");
            } else if owner_recording_built() {
                put_lit(b"  holder unrecorded (boot spinowner=1 for attribution)\n");
            } else {
                put_lit(
                    b"  holder unrecorded (build debug-lock-owner + boot spinowner=1 for attribution)\n",
                );
            }
        }
    }

    // --- (d1) all-CPU stacks (lockless sysrq-'l' twin) ---
    all_cpu_stacks();

    // --- (d2) the full task snapshot ---
    crate::debug::taskdump::dump_all_tasks("spinlock-deadlock");

    put_lit(b"=== dlspin last gasp complete ===\n");
    LAST_GASP_DONE.store(true, Ordering::Release);

    if spinpanic_on() {
        panic!(
            "spinlock deadlock cpu={} lock={:#x} waiter_ra={:#x}",
            cpu,
            lock_addr,
            waiter_ra
        );
    }
}

/// Peek at the holder CPU's current task: comm, pid, state, preempt
/// count and a bounded kernel-stack walk. This read is the ONLY way to
/// see a holder frozen with IF=0 — its own tick is dead, so every
/// same-CPU observer (watchdog, softlockup, cond_resched) is blind.
fn holder_peek(cpu: usize) {
    if cpu >= crate::config::MAX_CPUS {
        put_lit(b"  holder-cpu out of range\n");
        return;
    }
    let cur = crate::sched::sched::cpu_state(cpu).current;
    if cur.is_null() {
        put_lit(b"  holder-cpu ");
        put_dec(cpu as u64);
        put_lit(b": no current task\n");
        return;
    }
    // SAFETY: per-CPU current is null or a valid Task set by the
    // scheduler; read-only field access for diagnostics, the same
    // contract taskdump's per-CPU slots use (a task exiting on another
    // CPU concurrently is at worst missed, never half-read).
    let t = unsafe { &*cur };
    put_lit(b"  holder-cpu ");
    put_dec(cpu as u64);
    put_lit(b" current pid=");
    put_dec(t.pid() as u64);
    put_lit(b" comm=");
    put_comm(t);
    put_lit(b" state=");
    put_lit(crate::debug::taskdump::state_name(t.state().bits()).as_bytes());
    put_lit(b" preempt=");
    put_dec_i(t.preempt_count() as i64);
    put_lit(b" (irqsave-frozen class: IF=0, own tick dead - only a cross-CPU peek sees this)");
    put_kstack(t.thread().sp, 12, 128);
}

/// All-CPU stacks, sysrq-'l' shape, through the lockless writer. The
/// sysrq twin prints through printk (locks) — unusable from a spinning
/// context; this twin is what the last gasp can afford.
fn all_cpu_stacks() {
    put_lit(b"=== dlspin all-CPU stacks ===\n");
    let cs = crate::sched::sched::grq_diag_cpu_currents();
    for c in 0..crate::config::MAX_CPUS {
        let (pid, idle, cur) = cs[c];
        if cur.is_null() {
            put_lit(b"CPU");
            put_dec(c as u64);
            put_lit(b": no current task\n");
            continue;
        }
        // SAFETY: same per-CPU-current contract as holder_peek.
        let t = unsafe { &*cur };
        put_lit(b"CPU");
        put_dec(c as u64);
        put_lit(b": pid=");
        put_dec(pid as u64);
        put_lit(b" idle=");
        putc(if idle { b'y' } else { b'n' });
        put_lit(b" comm=");
        put_comm(t);
        put_lit(b" state=");
        put_lit(crate::debug::taskdump::state_name(t.state().bits()).as_bytes());
        put_kstack(t.thread().sp, 8, 96);
    }
}

// ============================================================================
// Synthetic deadlock (spindeadlock=1 / the unit-test ktest)
// ============================================================================

/// The wedge lock. A dedicated lock (never taken by any other
/// subsystem) keeps the synthetic wedge from masking a REAL deadlock's
/// bundle: the once-per-lock registry keys on the address.
static TEST_LOCK: crate::sync::spinlock::Spinlock<()> =
    crate::sync::spinlock::Spinlock::new(());

/// Set once the holder kthread froze holding TEST_LOCK.
static HELD: AtomicBool = AtomicBool::new(false);

/// Set once the waiter kthread completed its acquire (verdict ready).
static WAITER_DONE: AtomicBool = AtomicBool::new(false);

/// The waiter kthread's outcome (valid after WAITER_DONE).
static WAITER_FIRED: AtomicBool = AtomicBool::new(false);

extern "C" fn dlspin_holder_fn(_arg: *mut core::ffi::c_void) -> i32 {
    // Freeze recipe: acquire under irqsave and hold with interrupts
    // disabled until the waiter's last-gasp bundle completed (or the
    // 240 s safety cap — under single-thread TCG the waiter needs tens
    // of seconds to spin DEADLOCK_WARN_ITERS=100M iterations while the
    // softirq watchdogs' serial chatter eats host time; on KVM the
    // whole exchange is sub-second). While held, this CPU's tick is
    // dead and every same-CPU observer is blind — exactly the wedge
    // class the cross-CPU peek exists for.
    put_lit(b"DLSPIN-TEST: holder entered on cpu=");
    put_dec(crate::arch::smp::cpu_id() as u64);
    putc(b'\n');
    let g = TEST_LOCK.lock_irqsave();
    HELD.store(true, Ordering::Release);
    put_lit(b"DLSPIN-TEST: holder froze on cpu=");
    put_dec(crate::arch::smp::cpu_id() as u64);
    putc(b'\n');
    let t0 = crate::arch::cpu::read_time();
    let cap = 240 * crate::config::TIMER_CLOCK_FREQ_HZ as u64;
    let mut beat = 0u64;
    while !LAST_GASP_DONE.load(Ordering::Acquire) {
        let elapsed = crate::arch::cpu::read_time().saturating_sub(t0);
        if elapsed >= cap {
            break;
        }
        if elapsed / (2 * crate::config::TIMER_CLOCK_FREQ_HZ as u64) > beat {
            beat = elapsed / (2 * crate::config::TIMER_CLOCK_FREQ_HZ as u64);
            put_lit(b"DLSPIN-TEST: holder holding ");
            put_dec(beat * 2);
            put_lit(b"s\n");
        }
        core::hint::spin_loop();
    }
    drop(g); // unlock + preempt_enable + irq_restore — boot continues
    0
}

extern "C" fn dlspin_waiter_fn(_arg: *mut core::ffi::c_void) -> i32 {
    // Wait for the holder's freeze, then become the waiter: the
    // deadlock warn + bundle fire from inside RawSpinlock::lock's spin
    // loop. Holder and waiter can never share a CPU — the holder is
    // frozen on its own with preemption disabled, and this kthread
    // spins (preempt-disabled) wherever the scheduler put it.
    put_lit(b"DLSPIN-TEST: waiter entered on cpu=");
    put_dec(crate::arch::smp::cpu_id() as u64);
    putc(b'\n');
    let t0 = crate::arch::cpu::read_time();
    let cap = 20 * crate::config::TIMER_CLOCK_FREQ_HZ as u64;
    let mut beat = 0u64;
    while !HELD.load(Ordering::Acquire) {
        let elapsed = crate::arch::cpu::read_time().saturating_sub(t0);
        if elapsed >= cap {
            put_lit(b"DLSPIN-TEST: waiter HELD-wait timed out (20s)\n");
            WAITER_FIRED.store(false, Ordering::Release);
            WAITER_DONE.store(true, Ordering::Release);
            return 0;
        }
        if elapsed / (2 * crate::config::TIMER_CLOCK_FREQ_HZ as u64) > beat {
            beat = elapsed / (2 * crate::config::TIMER_CLOCK_FREQ_HZ as u64);
            put_lit(b"DLSPIN-TEST: waiter waiting for holder ");
            put_dec(beat * 2);
            put_lit(b"s\n");
        }
        crate::sched::cond_resched();
    }
    put_lit(b"DLSPIN-TEST: waiter spinning on cpu=");
    put_dec(crate::arch::smp::cpu_id() as u64);
    putc(b'\n');
    let g = TEST_LOCK.lock();
    drop(g);
    put_lit(b"DLSPIN-TEST: waiter acquired\n");
    WAITER_FIRED.store(WARN_COUNT.load(Ordering::Acquire) > 0, Ordering::Release);
    WAITER_DONE.store(true, Ordering::Release);
    0
}

/// Spawn the synthetic 2-CPU deadlock as two BACKGROUND kthreads
/// (holder + waiter) and return immediately. The wedge then overlaps
/// the rest of the unit-test suite instead of blocking it (the waiter
/// needs up to tens of seconds of spin iterations under single-thread
/// TCG); the verdict is collected by `test_deadlock_verdict` at suite
/// end. Returns false when the kthreads could not be spawned.
pub fn test_deadlock_start() -> bool {
    WARN_COUNT.store(0, Ordering::Relaxed);
    HELD.store(false, Ordering::Release);
    LAST_GASP_DONE.store(false, Ordering::Release);
    WAITER_DONE.store(false, Ordering::Release);
    WAITER_FIRED.store(false, Ordering::Release);

    let holder = crate::process::kthread::kernel_thread(
        dlspin_holder_fn,
        core::ptr::null_mut(),
        0,
        "dlspin-holder",
    );
    put_lit(b"DLSPIN-TEST: holder spawn -> ");
    if holder.is_some() { put_lit(b"ok"); } else { put_lit(b"FAIL"); }
    putc(b'\n');
    let waiter = crate::process::kthread::kernel_thread(
        dlspin_waiter_fn,
        core::ptr::null_mut(),
        0,
        "dlspin-waiter",
    );
    put_lit(b"DLSPIN-TEST: waiter spawn -> ");
    if waiter.is_some() { put_lit(b"ok"); } else { put_lit(b"FAIL"); }
    putc(b'\n');
    if holder.is_none() || waiter.is_none() {
        put_lit(b"DLSPIN-TEST: kthread spawn failed\n");
        return false;
    }
    // Task pointers for the verdict loop's re-kick insurance.
    // SAFETY-free plain stores of the live Task pointers returned by
    // kernel_thread; used only by test_deadlock_verdict's wake retries.
    HOLDER_TASK.store(
        holder.map(|t| t as *mut _ as usize).unwrap_or(0),
        Ordering::Release,
    );
    WAITER_TASK.store(
        waiter.map(|t| t as *mut _ as usize).unwrap_or(0),
        Ordering::Release,
    );
    put_lit(b"DLSPIN-TEST: spawned; nr_running=");
    put_dec(crate::sched::sched::GlobalRunQueue::grq_nr_running() as u64);
    put_lit(b" cpus=");
    put_dec(crate::arch::smp::num_started_cpus() as u64);
    putc(b'\n');
    true
}

/// Task pointers for the re-kick insurance (verdict loop).
static HOLDER_TASK: AtomicUsize = AtomicUsize::new(0);
static WAITER_TASK: AtomicUsize = AtomicUsize::new(0);

/// Collect the synthetic deadlock's verdict (suite end): bounded wait
/// for the waiter kthread to complete its acquire, then report whether
/// the last-gasp bundle fired.
///
/// Re-kick insurance (the x86 KVM proof lesson, 2026-11-08): the
/// kthreads' initial enqueue may lose its idle-wake — the resched IPI
/// fired while the target secondary was mid-park and was swallowed by
/// the park transition (the WAKE-REFUSED family the syscost census
/// tracks at ~32/boot). `Task::wake_up` cannot retry that pull: the
/// kthreads are already RUNNING and on the GRQ, and wake_up's
/// is_sleeping() gate turns it into a no-op — observed live as a
/// 90 s x86 boot where neither kthread ever ran its first instruction
/// while the boot CPU looped in this verdict. The retry therefore
/// REPEATS THE PULL, not the wake: resched every remote CPU so a
/// parked idle one re-polls the GRQ (NEED_RESCHED + IPI, Dekker-paired
/// with the idle loop — idempotent for a CPU already running). Every
/// 5 s while the exchange is unresolved.
pub fn test_deadlock_verdict() -> Option<bool> {
    let t0 = crate::arch::cpu::read_time();
    // Generous bound: waiter spin-up + 100M iterations under TCG.
    let cap = 300 * crate::config::TIMER_CLOCK_FREQ_HZ as u64;
    let mut beat = 0u64;
    while !WAITER_DONE.load(Ordering::Acquire) {
        let elapsed = crate::arch::cpu::read_time().saturating_sub(t0);
        if elapsed >= cap {
            return None;
        }
        if elapsed / (5 * crate::config::TIMER_CLOCK_FREQ_HZ as u64) > beat {
            beat = elapsed / (5 * crate::config::TIMER_CLOCK_FREQ_HZ as u64);
            put_lit(b"DLSPIN-TEST: verdict waiting ");
            put_dec(beat * 5);
            put_lit(b"s\n");
            if !HELD.load(Ordering::Acquire) {
                // The pull retry (see the doc comment): nudge every
                // remote started CPU. A parked idle CPU re-polls the
                // GRQ on the resched IPI and picks the queued kthread;
                // a busy CPU at worst takes an extra pick pass.
                let me = crate::arch::smp::cpu_id() as usize;
                for c in 0..crate::arch::smp::num_started_cpus() {
                    if c != me {
                        crate::sched::sched::resched_cpu(c);
                    }
                }
                // Belt and braces: re-deliver the wake too — covers a
                // kthread parked on its HELD wait whose sleep wake was
                // the lost one (idempotent for running tasks).
                for slot in [&HOLDER_TASK, &WAITER_TASK] {
                    let p = slot.load(Ordering::Acquire) as *mut crate::process::task::Task;
                    if !p.is_null() {
                        // SAFETY: the pointer came from kernel_thread's
                        // returned live Task; the kthread Task stays
                        // allocated until it exits (and nothing reaps a
                        // kthread of the immortal boot task during this
                        // window). A wake on a dead task is refused by
                        // the state check inside.
                        unsafe { crate::process::task::Task::wake_up(p) };
                    }
                }
            }
        }
        crate::sched::cond_resched();
    }
    Some(WAITER_FIRED.load(Ordering::Acquire))
}

/// Arm owner recording programmatically (the ktest's first step — full
/// holder attribution on every unit-test boot; the unit-test feature
/// pulls in debug-lock-owner).
#[cfg_attr(not(feature = "unit-test"), allow(dead_code))]
pub fn test_arm_spinowner() {
    #[cfg(feature = "debug-lock-owner")]
    SPINOWNER_ARMED.store(true, Ordering::Release);
}

/// Late-boot entry (after secondaries are online, ksoftirqd/hung_task
/// slot): drive the synthetic deadlock when `spindeadlock=1` asks for
/// it. Blocking variant — the proof boot waits for the exchange to
/// finish so the "resolved" line in the log proves the machine lived
/// through it. Unit-test builds are excluded — the ktest drives its
/// own instance from run_all_tests.
pub fn late_init() {
    #[cfg(not(feature = "unit-test"))]
    {
        if !SPINDEADLOCK_TEST.load(Ordering::Acquire) {
            return;
        }
        crate::pr_info!("debug: dlspin synthetic deadlock starting (spindeadlock=1)");
        if !test_deadlock_start() {
            return;
        }
        let verdict = test_deadlock_verdict();
        crate::pr_info!(
            "debug: dlspin synthetic deadlock resolved: last-gasp fired={:?}",
            verdict
        );
    }
    #[cfg(feature = "unit-test")]
    let _ = ();
}
