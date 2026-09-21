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

/// Snapshot every task in the PID hash. One line per task, plus a header
/// so the dump is self-describing in a log full of interleaved output.
pub fn dump_all_tasks(reason: &str) {
    puts("\n=== DFX TASK DUMP (");
    puts(reason);
    puts(") ===\n");

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
