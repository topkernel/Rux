//! Slow-kernel-entry audit (leg-33B, `dfx=slowtask`).
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

static mut SLOTS: [Slot; TABLE_N] = [const {
    Slot {
        pid: 0,
        sys: 0,
        epc: 0,
        state: 0,
        first_j: 0,
        last_j: 0,
        printed_j: 0,
    }
}; TABLE_N];

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
    if !crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::SlowTask) {
        return;
    }
    let round = ROUND.fetch_add(1, Ordering::Relaxed);
    if round % 4 != 0 {
        return;
    }
    let now = crate::drivers::timer::get_jiffies() as u64;
    // SAFETY: single-auditor (heartbeat CAS admits one CPU; the table
    // is only touched from this path).
    unsafe { audit(now) };
}

/// Collect this round's stuck entries (>= 8 s old), print them.
///
/// SAFETY: called only from `tick` (single auditor).
unsafe fn audit(now: u64) {
    // Age out stale slots (task exited / syscall changed): anything not
    // refreshed this round dies.
    if TABLE_USED.load(Ordering::Relaxed) {
        for s in SLOTS.iter_mut() {
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

    crate::sched::for_each_task(|task| {
        // SAFETY: diagnostic reads under the for_each_task contract.
        let tk = unsafe { &*task };
        let st_bits = tk.state().bits();
        let sleeping = st_bits & 0x3 != 0; // INTERRUPTIBLE | UNINTERRUPTIBLE
        if !sleeping {
            return;
        }
        let Some(top) = tk.kernel_stack_top_checked() else {
            return;
        };
        #[cfg(feature = "x86_64")]
        let (sys, epc, a0, a1) = {
            let regs = (top.wrapping_sub(
                crate::arch::x86_64::pt_regs::PT_REGS_SIZE,
            )) as *const crate::arch::x86_64::pt_regs::PtRegs;
            // SAFETY: entry-frame read of this task's own kernel stack.
            let r = unsafe { &*regs };
            (r.orig_rax as u64, r.rip as u64, r.rdi, r.rsi)
        };
        #[cfg(feature = "riscv64")]
        let (sys, epc, a0, a1) = {
            let regs = (top.wrapping_sub(
                crate::arch::riscv64::pt_regs::PT_REGS_SIZE,
            )) as *const crate::arch::riscv64::pt_regs::PtRegs;
            // SAFETY: entry-frame read of this task's own kernel stack.
            let r = unsafe { &*regs };
            (r.a7 as u64, r.epc as u64, r.a0 as u64, r.a1 as u64)
        };
        #[cfg(feature = "aarch64")]
        let (sys, epc, a0, a1) = {
            let regs = (top.wrapping_sub(
                crate::arch::aarch64::pt_regs::PT_REGS_SIZE,
            )) as *const crate::arch::aarch64::pt_regs::PtRegs;
            // SAFETY: entry-frame read of this task's own kernel stack.
            let r = unsafe { &*regs };
            (r.syscallno, r.pc, r.regs[0], r.regs[1])
        };
        if sys == 0 {
            // Not a syscall entry (interrupt frame) — skip.
            return;
        }
        let pid = tk.pid();
        // SAFETY: single-auditor table (heartbeat CAS admits one CPU;
        // SLOTS is only touched from this path).
        let (i, matched) = unsafe {
            let mut idx: Option<usize> = None;
            for (i, s) in SLOTS.iter().enumerate() {
                if s.pid == pid {
                    idx = Some(i);
                    break;
                }
            }
            match idx {
                Some(i) => (
                    i,
                    SLOTS[i].sys == sys && SLOTS[i].epc == epc,
                ),
                None => {
                    let free = SLOTS.iter().position(|s| s.pid == 0);
                    let Some(i) = free else { return };
                    let s = &mut SLOTS[i];
                    s.pid = pid;
                    s.sys = sys;
                    s.epc = epc;
                    s.state = st_bits;
                    s.first_j = now;
                    s.last_j = now;
                    s.printed_j = 0;
                    TABLE_USED.store(true, Ordering::Relaxed);
                    (i, true)
                }
            }
        };
        // SAFETY: single-auditor table.
        let slot = unsafe { &mut SLOTS[i] };
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
        let age_j = now.saturating_sub(slot.first_j);
        // Two-round persistence (>= 8 s) and a 32 s reprint cadence.
        if age_j >= 8 * 100 && now.saturating_sub(slot.printed_j) >= 32 * 100 {
            notes.push(Note {
                pid,
                sys,
                epc,
                state: st_bits,
                a0,
                a1,
                age_j,
            });
            slot.printed_j = now;
        }
    });

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
        // line; the rest are counted for the summary.
        if !fs && !(n.pid == oldest_pid && n.sys == oldest_sys) {
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
            let t = crate::sched::find_task_by_pid(n.pid);
            if !t.is_null() {
                // SAFETY: diagnostic comm read; task found by pid.
                let c = unsafe { (*t).comm() };
                let n = c.len().min(comm_buf.len());
                comm_buf[..n].copy_from_slice(&c[..n]);
                comm_len = comm_buf[..n].iter().position(|&b| b == 0).unwrap_or(n);
            } else {
                comm_len = 0;
            }
        }
        let comm = core::str::from_utf8(&comm_buf[..comm_len]).unwrap_or("?");
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
        crate::pr_warn!("SLOWTASK-SUMMARY lockfamily={} round_j={}", lock_lines, now);
    }
}

/// Print up to 24 parked-chain kernel-text words from the task's saved
/// sp upward (the wait chain; symbolize offline).
fn print_parked_chain(pid: u32, epc: u64) {
    // SAFETY: pid lookup + diagnostic reads below are read-only.
    let t = unsafe { crate::sched::find_task_by_pid(pid) };
    if t.is_null() {
        return;
    }
    // SAFETY: diagnostic reads of the task's fields and its pinned
    // kernel-stack pages (the Task exists).
    let tk = unsafe { &*t };
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
    let p = sp as *const u64;
    let mut words: [u64; 24] = [0; 24];
    let mut n = 0usize;
    for k in 0..640usize {
        // SAFETY: diagnostic read inside the task's own kernel-stack
        // window; bounds checked above.
        let v = unsafe { core::ptr::read_volatile(p.add(k)) };
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
