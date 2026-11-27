//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Slow-kernel-entry audit (leg-33B, `rux:slowtask (retired dfx=slowtask)`).
//!
//! sc329B adjudication gap: the TXN-HANG lines name the SERVER thread of
//! a hung transaction and its LAST kernel entry (sys/rip), but nothing
//! attributes tasks that stall inside NON-binder syscalls — and the
//! foundation reader-exhaustion cascade had its leaves exactly there
//! (hiview OS_IPC_1_680 in pwrite64 for 30+ s, powermgr in an
//! uninterruptible open, foundation tid 808 in ftruncate). Every layer
//! above those leaves (futex lock waits, BINDER_WRITE_READ reply waits,
//! TXN-HANG growth, watchdog SIGABRT) is downstream.
//!
//! Contract: called from the 1 Hz timer-softirq heartbeat slot (same
//! discipline as txn_hang_audit — the heartbeat compare_exchange admits
//! exactly one CPU). Every 4th call (~4 s), walk all tasks once and
//! snapshot (pid, syscall-nr, entry-epc, state) of every SLEEPING task
//! that has entered the kernel. A tuple that persists across two rounds
//! (>= 8 s on the same kernel entry) is STUCK; fs-family syscalls print
//! a full SLOWTASK line with the parked kernel-stack chain; lock-family
//! waits (futex/epoll/poll) collapse into a one-line summary unless one
//! of them is the single oldest entry of the round. Stack words come
//! from the same diagnostic windows as TXN-HANG-STK (sleep_snapshot
//! contract, sc261); symbolize offline with addr2line.
//!
//! All state lives in this module's static table (single-auditor,
//! no locks needed); zero cost when the switch is off.

#![forbid(unsafe_code)]
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

const TABLE_N: usize = 192;

/// One tracking slot: the entry tuple and when it was first/last seen.
struct Slot {
    pid: u32,
    sys: u64,
    epc: u64,
    state: u32,
    first_j: u64,
    last_j: u64,
    printed_j: u64,
}

// Round table under a spinlock: the heartbeat CAS admits ONE auditor,
// so the lock is uncontended in the healthy case — it exists so the
// slot array's exclusivity is type-checked, not review-enforced.
static SLOTS: crate::sync::spinlock::Spinlock<[Slot; TABLE_N]> =
    crate::sync::spinlock::Spinlock::new([const {
        Slot {
            pid: 0,
            sys: 0,
            epc: 0,
            state: 0,
            first_j: 0,
            last_j: 0,
            printed_j: 0,
        }
    }; TABLE_N]);

/// Round counter (incremented per audit fire; a fire is every ~4 s).
static ROUND: AtomicU32 = AtomicU32::new(0);
/// Whether any SLOTS cell was ever written (skip the clear sweep until
/// then).
static TABLE_USED: AtomicBool = AtomicBool::new(false);
/// Total SLOWTASK lines emitted this boot (hard budget).
static LINES: AtomicU64 = AtomicU64::new(0);

/// fs-family syscall numbers (arch tables differ).
#[cfg(feature = "x86_64")]
const FS_FAMILY: &[u64] = &[
    0, 1, 2, 3, // read write open close
    4, 5, 6, // stat fstat lstat
    16, // ioctl (BINDER_WRITE_READ sits here — binder reads matter too)
    17, 18, 19, 20, // pread64 pwrite64 readv writev
    76, 77, // truncate ftruncate
    74, 75, // fsync fdatasync
    32, 33, // dup dup2 (fd table walks)
    87, 88, // unlink rename (x86_64 numbers)
    83, 84, 85, 86, // mkdir rmdir creat link
    257, // openat
    322, // sync_file_range
];
#[cfg(any(feature = "riscv64", feature = "aarch64"))]
const FS_FAMILY: &[u64] = &[
    63, 64, // read write
    43, 57, // close dup3 (generic)
    49, 50, // (spare)
    29, 30, // ioctl (generic 29)
    67, 68, // pread64 pwrite64 (generic 67/68)
    65, 66, // readv writev (generic 65/66)
    45, 46, // truncate ftruncate (generic)
    82, 83, // fsync fdatasync (generic)
    34, 35, // mkdirat unlinkat (generic)
    38, 39, // renameat renameat2 (generic)
    56, // openat (generic)
    62, // lseek
];

fn fs_family(sys: u64) -> bool {
    FS_FAMILY.contains(&sys)
}

/// Audit entry point (timer heartbeat, ~1 Hz caller; fires every 4th).
pub fn tick() {
    if !crate::trace::EVENT_SLOWTASK.armed() {
        return;
    }
    let round = ROUND.fetch_add(1, Ordering::Relaxed);
    if round % 4 != 0 {
        return;
    }
    let now = crate::drivers::timer::get_jiffies() as u64;
    // Single-auditor (heartbeat CAS admits one CPU; the table is only
    // touched from this path). audit is plain safe code.
    audit(now);
}

/// Collect this round's stuck entries (>= 8 s old), print them.
///
/// Called only from `tick` (single auditor, heartbeat CAS).
fn audit(now: u64) {
    // Age out stale slots (task exited / syscall changed): anything not
    // refreshed this round dies.
    if TABLE_USED.load(Ordering::Relaxed) {
        for s in SLOTS.lock_irqsave().iter_mut() {
            if s.pid != 0 && now.saturating_sub(s.last_j) > 20 * 100 {
                s.pid = 0;
            }
        }
    }

    // Round walk: match or insert.
    struct Note {
        pid: u32,
        sys: u64,
        epc: u64,
        state: u32,
        a0: u64,
        a1: u64,
        age_j: u64,
    }
    let mut notes: alloc::vec::Vec<Note> = alloc::vec::Vec::new();

    // lb325 wedge: this audit runs at the timer-softirq heartbeat (IF=0,
    // non-preemptible). The BLOCKING task walk deadlocks when the snapshot
    // reaches a bucket whose lock an interrupted task-context writer still
    // holds (self-deadlock by interrupt). Try-walk: buckets held at
    // snapshot time are skipped; a tuple missed one round simply
    // re-baselines next round (the >=8 s persistence filter is unaffected).
    let skipped = crate::sched::for_each_task_try(|task| {
        // The walk hands &Task under the bucket lock — plain reads.
        let tk = task;
        let st_bits = tk.state().bits();
        // Lost-park all-task census (rux:lostpark (retired dfx=lostpark)): one unfiltered row
        // per task every audit round while wedged-hunting — answers
        // "which bookkeeping class does the vanished task rest in"
        // (runqueue, slots, wakeable state, pid-hash presence).
        if crate::trace::EVENT_LOSTPARK.armed() && tk.pid() != 0
            && tk.pid() != u32::MAX
        {
            static LPALL_N: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            if LPALL_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
                < 24
            {
                let claim = tk.running_on_cpu.load(core::sync::atomic::Ordering::Acquire);
                crate::pr_warn!(
                    "LP-CENSUS pid={} state={:#x} pc={:#x} oncpu={} pk={} cl={} onrq={} j={}",
                    tk.pid(),
                    st_bits,
                    tk.preempt_count() as u32 as u64,
                    tk.on_cpu() as u64,
                    tk.pick_state() as u64,
                    claim as i64,
                    tk.sched_entity().is_on_rq() as u64,
                    now
                );
                // The parked chain names the exact park site (the
                // unregistered-park signature hunt).
                print_parked_chain(tk.pid(), 0);
            }
        }
        let sleeping = st_bits & 0x3 != 0; // INTERRUPTIBLE | UNINTERRUPTIBLE
        if !sleeping {
            // Lost-park census (fix-lostpark, rux:lostpark (retired dfx=lostpark)): a RUNNING
            // task owned by no CPU slot is the R46 phantom family — on
            // x86_64 the idle-loop harvester is riscv64-gated, so nothing
            // else ever names or heals it. Report it with the same
            // aging/chain discipline so the wedge capture names the
            // vanished poster.
            if crate::trace::EVENT_LOSTPARK.armed() && tk.pid() != 0
                && tk.pid() != u32::MAX
            {
                let owned = (0..crate::config::MAX_CPUS)
                    .any(|c| crate::sched::sched::cpu_state(c).current as usize == task as *const _ as usize);
                if !owned {
                    notes.push(Note {
                        pid: tk.pid(),
                        sys: 0xFFFF_FFFF_0000_0001, // marker: non-sleeping, slotless
                        epc: 0,
                        state: st_bits,
                        a0: tk.preempt_count() as u64,
                        a1: (tk.on_cpu() as u64) | ((owned as u64) << 8),
                        age_j: 0,
                    });
                    print_parked_chain(tk.pid(), 0);
                }
            }
            return;
        }
        let Some(top) = tk.kernel_stack_top_checked() else {
            return;
        };
        let _ = top;
        // Entry-frame read through the typed shared view.
        let Some(r) = tk.pt_regs_ref() else { return };
        #[cfg(feature = "x86_64")]
        let (sys, epc, a0, a1) = (r.orig_rax as u64, r.rip as u64, r.rdi, r.rsi);
        #[cfg(feature = "riscv64")]
        let (sys, epc, a0, a1) = (r.a7 as u64, r.epc as u64, r.a0 as u64, r.a1 as u64);
        #[cfg(feature = "aarch64")]
        let (sys, epc, a0, a1) = (r.syscallno as u64, r.pc as u64, r.regs[0] as u64, r.regs[1] as u64);
        if sys == 0 {
            // Not a syscall entry (interrupt frame) — skip, except for
            // the lost-park census (rux:lostpark (retired dfx=lostpark)): a USER task parked
            // under an interrupt frame never re-enters its wait loop's
            // recheck and no wake path names it — the vanished-task
            // signature of the ping-pong wedge. Name it once per audit
            // round with its parked chain.
            if crate::trace::EVENT_LOSTPARK.armed() && tk.pid() != 0
                && tk.pid() != u32::MAX
            {
                notes.push(Note {
                    pid: tk.pid(),
                    sys: 0xFFFF_FFFF_0000_0002, // marker: sleeping, IRQ frame
                    epc: 0,
                    state: st_bits,
                    a0: tk.preempt_count() as u64,
                    a1: tk.on_cpu() as u64,
                    age_j: 0,
                });
                print_parked_chain(tk.pid(), 0);
            }
            return;
        }
        let pid = tk.pid();
        // Single-auditor table (heartbeat CAS admits one CPU); the
        // spinlock makes the exclusivity type-checked. The lock is
        // dropped before any printing.
        let (i, matched, first_j) = {
            let mut table = SLOTS.lock_irqsave();
            let mut idx: Option<usize> = None;
            for (i, s) in table.iter().enumerate() {
                if s.pid == pid {
                    idx = Some(i);
                    break;
                }
            }
            match idx {
                Some(i) => {
                    let slot = &mut table[i];
                    let matched = slot.sys == sys && slot.epc == epc;
                    if matched {
                        slot.last_j = now;
                        slot.state = st_bits;
                    } else {
                        // Entry changed since last round — restart aging.
                        slot.sys = sys;
                        slot.epc = epc;
                        slot.state = st_bits;
                        slot.first_j = now;
                        slot.last_j = now;
                    }
                    (i, matched, slot.first_j)
                }
                None => {
                    let Some(i) = table.iter().position(|s| s.pid == 0) else {
                        return;
                    };
                    let s = &mut table[i];
                    s.pid = pid;
                    s.sys = sys;
                    s.epc = epc;
                    s.state = st_bits;
                    s.first_j = now;
                    s.last_j = now;
                    s.printed_j = 0;
                    TABLE_USED.store(true, Ordering::Relaxed);
                    (i, true, s.first_j)
                }
            }
        };
        let age_j = now.saturating_sub(first_j);
        // Two-round persistence (>= 8 s) and a 32 s reprint cadence.
        let printed_j = SLOTS.lock_irqsave()[i].printed_j;
        if age_j >= 8 * 100 && now.saturating_sub(printed_j) >= 32 * 100 {
            notes.push(Note {
                pid,
                sys,
                epc,
                state: st_bits,
                a0,
                a1,
                age_j,
            });
            SLOTS.lock_irqsave()[i].printed_j = now;
        }
    });
    if skipped > 0 {
        // Partial snapshot (a bucket was mid-write): never fatal, but the
        // round's persistence filter saw fewer tasks — say so once.
        static SKIP_N: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        if SKIP_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 8 {
            crate::pr_warn!(
                "SLOWTASK: round snapshot partial — {} pid-hash bucket(s) held, skipped",
                skipped
            );
        }
    }

    if notes.is_empty() {
        return;
    }
    // fs-family first, then by age.
    notes.sort_by(|a, b| {
        let fa = fs_family(a.sys);
        let fb = fs_family(b.sys);
        fb.cmp(&fa).then(b.age_j.cmp(&a.age_j))
    });
    // Identity of the round's single oldest entry (a lock-family wait
    // earns one full line only when it is THE oldest — its stack usually
    // names the lock holder's wait site).
    let oldest_pid = notes[0].pid;
    let oldest_sys = notes[0].sys;

    let mut lock_lines = 0usize;
    for n in notes.iter().take(20) {
        let fs = fs_family(n.sys);
        // Lock-family waits: only the single oldest entry gets a full
        // line; the rest are counted for the summary. Lost-park census
        // markers (high sentinel word) always print — they are one-shot
        // wedge captures, not recurring lock waits.
        let is_lp_marker = n.sys >> 32 == 0xFFFF_FFFF;
        if !fs && !(n.pid == oldest_pid && n.sys == oldest_sys) && !is_lp_marker {
            lock_lines += 1;
            continue;
        }
        if LINES.load(Ordering::Relaxed) > 400 {
            break;
        }
        LINES.fetch_add(1, Ordering::Relaxed);
        // comm into a fixed buffer (no String allocation in the audit
        // path; pr_warn! takes the &str).
        let mut comm_buf = [0u8; 16];
        let comm_len;
        {
            if let Some(t) = crate::process::task_ref::TaskBorrow::lookup(n.pid) {
                let c = t.comm();
                let n = c.len().min(comm_buf.len());
                comm_buf[..n].copy_from_slice(&c[..n]);
                comm_len = comm_buf[..n].iter().position(|&b| b == 0).unwrap_or(n);
            } else {
                comm_len = 0;
            }
        }
        let comm = core::str::from_utf8(&comm_buf[..comm_len]).unwrap_or("?");
        crate::trace::trace_slowtask(0, n.pid, n.sys, n.epc);
        crate::pr_warn!(
            "SLOWTASK pid={} comm='{}' state={:#x} sys={} epc={:#x} a0={:#x} a1={:#x} age={}s",
            n.pid,
            comm,
            n.state,
            n.sys,
            n.epc,
            n.a0,
            n.a1,
            n.age_j / 100
        );
        // Parked-chain words (sp-up window, deepened leg-33B): 12 words
        // per line, two lines, kernel-text addresses only.
        print_parked_chain(n.pid, n.epc);
    }
    if lock_lines > 0 {
        crate::trace::trace_slowtask(1, 0, lock_lines as u64, now);
        crate::pr_warn!("SLOWTASK-SUMMARY lockfamily={} round_j={}", lock_lines, now);
    }
}

/// Print up to 24 parked-chain kernel-text words from the task's saved
/// sp upward (the wait chain; symbolize offline).
fn print_parked_chain(pid: u32, epc: u64) {
    // TaskBorrow's short-read contract (diagnostic, no yield below).
    let tk = match crate::process::task_ref::TaskBorrow::lookup(pid) {
        Some(t) => t,
        None => return,
    };
    let Some(top) = tk.kernel_stack_top_checked() else {
        return;
    };
    #[cfg(feature = "x86_64")]
    extern "C" {
        static __stext: u8;
        static __etext: u8;
    }
    #[cfg(not(feature = "x86_64"))]
    extern "C" {
        static _stext: u8;
        static _etext: u8;
    }
    #[cfg(feature = "x86_64")]
    let (lo, hi) = (
        core::ptr::addr_of!(__stext) as u64,
        core::ptr::addr_of!(__etext) as u64,
    );
    #[cfg(not(feature = "x86_64"))]
    let (lo, hi) = (
        core::ptr::addr_of!(_stext) as u64,
        core::ptr::addr_of!(_etext) as u64,
    );
    let is_text = |v: u64| v >= lo && v < hi && (v & 1) == 0 && v != epc;
    let base = top.wrapping_sub(crate::config::KERNEL_STACK_SIZE) as u64;
    let sp = tk.thread().sp as u64;
    if sp < base || sp + 8 > top as u64 || (sp & 7) != 0 {
        return;
    }
    // Upward walk of the task's own stack through the bounds-checked
    // window (INV-WINDOW-*).
    let win = crate::safety::frame::KernelStackWindow::new(
        top - crate::config::KERNEL_STACK_SIZE,
        top,
    );
    let mut words: [u64; 24] = [0; 24];
    let mut n = 0usize;
    for k in 0..640usize {
        let v = match win.and_then(|w| w.read_u64((sp as usize) + k * 8)) {
            Some(v) => v,
            None => break,
        };
        if is_text(v) {
            words[n] = v;
            n += 1;
            if n == words.len() {
                break;
            }
        }
    }
    if n == 0 {
        return;
    }
    let mut line = alloc::string::String::new();
    use core::fmt::Write as _;
    let _ = write!(line, "SLOWTASK-STK pid={}", pid);
    for (i, w) in words.iter().take(n).enumerate() {
        if i == 12 {
            crate::pr_warn!("{}", line);
            line.clear();
            let _ = write!(line, "SLOWTASK-STK2 pid={}", pid);
        }
        let _ = write!(line, " {w:#x}");
    }
    crate::pr_warn!("{}", line);
}
