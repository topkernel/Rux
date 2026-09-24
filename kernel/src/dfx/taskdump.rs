//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Task-state snapshot for wedge diagnosis.
//!
//! Prints one line per task (pid, state, policy, comm) over the SBI
//! console — deliberately NOT printk, because the usual caller is the
//! spinlock deadlock watchdog spinning with local IRQs off and possibly
//! with locks the printk path itself needs.
//!
//! Gated by the `dfx=watchdog` / `dfx=taskdump` runtime switches (see
//! switches.rs); also available on demand through the UART trigger when
//! `taskdump` is on.

use core::sync::atomic::Ordering;

fn putc(c: u8) {
    // SAFETY: SBI legacy console_putchar is safe to call from any context.
    unsafe { sbi_rt::legacy::console_putchar(c as usize); }
}

fn puts(s: &str) {
    for &b in s.as_bytes() {
        putc(b);
    }
}

fn put_hex(v: u64) {
    let mut shift = 64;
    while shift > 0 {
        shift -= 4;
        let nibble = (v >> shift) & 0xF;
        let c = if nibble < 10 { b'0' + nibble as u8 } else { b'a' + (nibble - 10) as u8 };
        putc(c);
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

/// DFX raw diagnostic line (SBI console, no locks) for other subsystems'
/// one-shot probes — same discipline as the dump itself.
pub fn taskdump_raw_line(s: &[u8]) {
    for &b in s {
        putc(b);
    }
}

/// DFX decimal helper matching taskdump_raw_line.
pub fn taskdump_dec(v: u64) {
    put_dec(v);
}

/// Snapshot every task in the PID hash. One line per task, plus a header
/// so the dump is self-describing in a log full of interleaved output.
pub fn dump_all_tasks(reason: &str) {
    puts("\n=== DFX TASK DUMP (");
    puts(reason);
    puts(") ===\n");

    // 4-thread hang hunt: the idle fast path trusts grq_nr_running() (the
    // SUM of the per-class queue depths). Print it against the atomic and
    // the per-CPU current slots so a counter/queue divergence is visible in
    // one snapshot.
    {
        let (nr, cfs, rt, dl) = crate::sched::sched::grq_diag_counts();
        puts("grq: nr_running=");
        put_dec(nr as u64);
        puts(" cfs=");
        put_dec(cfs);
        puts(" rt=");
        put_dec(rt);
        puts(" dl=");
        put_dec(dl);
        let cur = crate::sched::sched::grq_diag_cpu_currents();
        puts(" cpu[pid/idle]:");
        for c in 0..crate::config::MAX_CPUS {
            putc(b' ');
            if cur[c].0 == u32::MAX {
                puts("-");
            } else {
                put_dec(cur[c].0 as u64);
            }
            putc(b'/');
            putc(if cur[c].1 { b'i' } else { b'b' });
        }
        putc(b'\n');
        // Backtrace each CPU's current (even hash-invisible ones — the
        // post-exit spinner family). Same guard discipline as the per-task
        // bt below.
        #[cfg(feature = "dfx-taskdump-bt")]
        for c in 0..crate::config::MAX_CPUS {
            let t = cur[c].2 as *mut crate::process::task::Task;
            if t.is_null() {
                continue;
            }
            // SAFETY: diagnostic read of stable fields on the per-CPU current
            // Task; the idle tasks and any hash-invisible exiting tasks are
            // still valid allocated Task objects.
            let (pid, st, preempt) = unsafe {
                ((*t).pid(), (*t).state().bits(), (*t).preempt_count())
            };
            puts("cpu");
            put_dec(c as u64);
            puts(" cur pid=");
            put_dec(pid as u64);
            puts(" state=");
            puts(state_name(st));
            puts(" preempt=");
            let pv = preempt as u64;
            if (pv as i64) < 0 {
                puts("-");
                put_dec((pv as i64).unsigned_abs());
            } else {
                put_dec(pv);
            }
            // Last user epc (pt_regs at kernel-stack top) — a userland
            // tight-loop vs mid-exit distinction.
            {
                let pr = unsafe { (*t).pt_regs() };
                if !pr.is_null() {
                    let epc = unsafe { (*pr).epc };
                    if epc > 0x1000 && epc < 0x0000_4000_0000_0000 {
                        puts(" uepc=");
                        put_hex(epc);
                    }
                }
            }
            puts(" @0x");
            put_hex(t as u64);
            {
                let sp = unsafe { (*t).thread().sp };
                if (sp >> 48) == 0xffff && (sp & 7) == 0 && sp > 0x1000 {
                    puts(" sp=");
                    put_hex(sp);
                    let base = sp as *const u64;
                    let mut shown = 0;
                    for k in 0..40 {
                        // SAFETY: diagnostic read within the task's own kernel
                        // stack pages (mapped while the Task exists).
                        let v = unsafe { core::ptr::read_volatile(base.wrapping_sub(k)) };
                        if v >= 0xffffffff80000000 && v < 0xffffffff80200000 && (v & 1) == 0 {
                            putc(b' ');
                            put_hex(v);
                            shown += 1;
                            if shown >= 6 { break; }
                        }
                    }
                }
            }
            putc(b'\n');
        }
    }

    let mut count: usize = 0;
    // Non-blocking iteration: this dump runs from the spinlock deadlock
    // watchdog (spinning, IRQs off) or the UART RX magic — exactly the
    // contexts where another CPU may be stuck HOLDING a pid-hash bucket
    // lock (the kill-broadcast / OOM paths call send_signal → wake_up
    // inside the for_each callback). Blocking on a bucket lock would wedge
    // the diagnostic itself, so busy buckets are skipped and the partial
    // snapshot is labeled below.
    //
    // SAFETY: pid_hash_for_each_task_try pins nothing and blocks on nothing;
    // the callback only reads stable Task fields (pid/state/policy/comm) of
    // tasks that remain in the hash while we hold the bucket lock. A task
    // exiting on another CPU concurrently may be missed (fine for a
    // snapshot) but can never be half-printed: the slot is freed only after
    // pid_hash_remove, which the iterator's bucket locks serialize against.
    let skipped = unsafe {
        crate::process::pid_hash::pid_hash_for_each_task_try(|task_ptr| {
            let t = &*task_ptr;
            count += 1;
            puts("task @0x");
            put_hex(task_ptr as u64);
            puts(" pid=");
            put_dec(t.pid() as u64);
            puts(" state=");
            puts(state_name(t.state().bits()));
            puts(" ti_cpu=");
            put_dec(t.ti_cpu() as u64);
            puts(" on_cpu=");
            put_dec(t.on_cpu() as u64);
            // Authoritative linked-state (tree/list scan, not the flag) —
            // settles flag-desync vs really-off-queue in one shot.
            let linked = unsafe {
                let g = crate::sched::sched::grq_diag_cfs_linked(task_ptr);
                g
            };
            puts(" linked=");
            put_dec(linked as u64);
            puts(" on_rq=");
            put_dec(t.sched_entity().on_rq.load(Ordering::Relaxed) as u64);
            puts(" affinity=0x");
            put_hex(t.cpus_allowed() as u64);
            puts(" vruntime=");
            put_dec(t.sched_entity().get_vruntime());
            puts(" policy=");
            puts(policy_name(t));
            puts(" comm=");
            let comm = t.comm();
            let name = comm.split(|&b| b == 0).next().unwrap_or(b"?");
            for &b in name.iter().take(16) {
                if b >= 0x20 && b < 0x7f {
                    putc(b);
                } else {
                    putc(b'.');
                }
            }
            putc(b'\n');
            #[cfg(feature = "dfx-taskdump-bt")]
            {
                // R38: coarse stack backtrace — print thread.sp and the first
                // code-region words scanning down the kernel stack (return
                // addresses), for wedged-sleeper and RUNNING-but-unscheduled
                // diagnosis. Guarded to plausible kernel-stack pointers only.
                // Feature-gated: the extra code shifts binary layout and
                // measurably widens the heap-lock race window — enable only
                // when hunting (DFX zero-cost-when-off principle).
                // Hunt addition: last user epc/sp — pinpoints the user
                // instruction a sleeper is blocked at (musl futex etc.).
                {
                    let pr = t.pt_regs();
                    if !pr.is_null() {
                        // SAFETY: pt_regs is the task's own kernel-stack-top
                        // trap frame; read-only diagnostic access.
                        let (epc, usp) = unsafe { ((*pr).epc, (*pr).sp) };
                        if epc > 0x1000 && epc < 0x0000_4000_0000_0000 {
                            puts(" uepc=");
                            put_hex(epc);
                        }
                        if usp > 0x1000 && usp < 0x0000_4000_0000_0000 {
                            puts(" usp=");
                            put_hex(usp);
                        }
                        // Last syscall args (a7=nr, orig_a0..a2) — identifies
                        // the futex word a sleeper is parked on (a0 holds the
                        // return slot; the true first arg is orig_a0).
                        let (a7, a0, a1, a2) =
                            unsafe { ((*pr).a7, (*pr).orig_a0, (*pr).a1, (*pr).a2) };
                        if a7 > 0 && a7 < 500 {
                            puts(" nr=");
                            put_dec(a7);
                            puts(" a0=");
                            put_hex(a0);
                            puts(" a1=");
                            put_hex(a1);
                            puts(" a2=");
                            put_hex(a2);
                        }
                    }
                }
                let sp = t.thread().sp;
                if (sp >> 48) == 0xffff && (sp & 7) == 0 && sp > 0x1000 {
                    puts("     sp=");
                    put_hex(sp);
                    let base = sp as *const u64;
                    let mut shown = 0;
                    for k in 0..40 {
                        // SAFETY: diagnostic read within the task's own kernel
                        // stack pages (mapped while the Task exists).
                        let v = unsafe { core::ptr::read_volatile(base.wrapping_sub(k)) };
                        if v >= 0xffffffff80000000 && v < 0xffffffff80200000 && (v & 1) == 0 {
                            putc(b' ');
                            put_hex(v);
                            shown += 1;
                            if shown >= 6 { break; }
                        }
                    }
                    puts("\n");
                }
            }
        })
    };
    puts("=== ");
    put_dec(count as u64);
    puts(" tasks ===\n");
    if skipped > 0 {
        puts("warning: ");
        put_dec(skipped as u64);
        puts(" pid-hash buckets were locked (stuck holder?) and skipped\n");
    }
}

fn state_name(bits: u32) -> &'static str {
    use crate::process::task::TaskState as S;
    match bits {
        S::RUNNING => "RUNNING",
        // R52: a task between new_task_at and its first enqueue. Legit
        // only transiently inside do_clone/kernel_thread; a PERSISTENT
        // NEW+ti_cpu=-1 line means the creating fork never reached its
        // enqueue (or its CPU froze mid-fork) — while RUNNING+ti_cpu=-1
        // is now impossible from the constructor and points at an
        // out-of-protocol state-word write.
        S::TASK_NEW => "NEW(built)",
        S::INTERRUPTIBLE => "INTERRUPTIBLE(sleep)",
        S::UNINTERRUPTIBLE => "UNINTERRUPTIBLE(dsleep)",
        S::STOPPED => "STOPPED",
        S::TRACED => "TRACED",
        S::ZOMBIE => "ZOMBIE",
        S::DEAD => "DEAD",
        _ => {
            if bits & S::ZOMBIE != 0 {
                "ZOMBIE(mixed)"
            } else if bits & S::DEAD != 0 {
                "DEAD(mixed)"
            } else if bits & S::UNINTERRUPTIBLE != 0 {
                "UNINTERRUPTIBLE(mixed)"
            } else if bits & S::INTERRUPTIBLE != 0 {
                "INTERRUPTIBLE(mixed)"
            } else if bits & S::STOPPED != 0 {
                "STOPPED(mixed)"
            } else {
                "UNKNOWN"
            }
        }
    }
}

fn policy_name(t: &crate::process::task::Task) -> &'static str {
    use crate::process::task::SchedPolicy;
    match t.policy() {
        SchedPolicy::Normal => "CFS",
        SchedPolicy::Batch => "BATCH",
        SchedPolicy::Idle => "IDLE",
        SchedPolicy::Fifo => "FIFO",
        SchedPolicy::Rr => "RR",
        SchedPolicy::Deadline => "DL",
    }
}
