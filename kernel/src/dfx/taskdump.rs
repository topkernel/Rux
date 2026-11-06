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
    unsafe { crate::console::putchar_no_lock(c); }
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

/// DFX: replay the forensic syscall ring for one pid (last 48 entries).
#[cfg(feature = "dfx-futex-trace")]
pub fn dump_syscall_ring_for(pid: u32) {    use crate::syscall::dispatch::{SYSCALL_CURSOR, SYSCALL_RING};
    let cur = SYSCALL_CURSOR.load(core::sync::atomic::Ordering::Relaxed);
    let len = SYSCALL_RING.len();
    taskdump_raw_line(b"FTX-RING pid=");
    taskdump_dec(pid as u64);
    taskdump_raw_line(b"\n");
    // Walk backwards from the oldest surviving entry to the newest.
    let start = if cur > len { cur - len } else { 0 };
    let mut shown = 0;
    for i in start..cur {
        let e = &SYSCALL_RING[i % len];
        let (p, nr, a0, a1, ret) = (
            e.pid.load(core::sync::atomic::Ordering::Relaxed),
            e.nr.load(core::sync::atomic::Ordering::Relaxed),
            e.a0.load(core::sync::atomic::Ordering::Relaxed),
            e.a1.load(core::sync::atomic::Ordering::Relaxed),
            e.ret.load(core::sync::atomic::Ordering::Relaxed),
        );
        if p == 0 || p != pid {
            continue;
        }
        taskdump_raw_line(b"FTX-SYS nr=");
        taskdump_dec(nr as u64);
        taskdump_raw_line(b" a0=");
        taskdump_dec(a0);
        taskdump_raw_line(b" ret=");
        taskdump_dec(ret as i64 as u64);
        taskdump_raw_line(b"\n");
        shown += 1;
        if shown >= 48 {
            break;
        }
    }
    if shown == 0 {
        taskdump_raw_line(b"FTX-RING-EMPTY\n");
    }
}

/// DFX: read one user u32 through a task's page tables (diagnostic).
#[cfg(feature = "dfx-futex-trace")]
fn dfx_task_read_u32(t: &crate::process::Task, va: u64) -> Option<u32> {
    use crate::arch::mm::mm_ops::PageTableWalker;
    use crate::arch::mm::memory_layout::phys_to_virt;
    let as_arc = t.address_space_arc()?;
    let root = as_arc.root_ppn();
    // SAFETY: diagnostic walk + linear-map read of the task's own mm.
    unsafe {
        let (ppn, _) = PageTableWalker::walk(root, va)?;
        let base = phys_to_virt(crate::arch::mm::memory_layout::PhysAddr(
            (ppn << 12) as u64,
        ));
        Some(core::ptr::read_volatile(
            (base.as_usize() + (va & 0xfff) as usize) as *const u32,
        ))
    }
}

/// DFX: one-line periodic snapshot of a tracked user word.
#[cfg(feature = "dfx-futex-trace")]
fn dfx_track_word(t: &crate::process::Task, va: u64) {
    match dfx_task_read_u32(t, va) {
        Some(v) => {
            taskdump_raw_line(b"FTX-TRACK u=");
            taskdump_dec(va);
            taskdump_raw_line(b" v=");
            taskdump_dec(v as u64);
            taskdump_raw_line(b"\n");
        }
        None => {}
    }
}

/// DFX: dump a parked task's user stack words that look like code pointers,
/// read through the task's own page tables (its mm is not active here).
#[cfg(feature = "dfx-futex-trace")]
fn dfx_dump_user_stack(t: &crate::process::Task, usp: u64) {
    use crate::arch::mm::mm_ops::PageTableWalker;
    use crate::arch::mm::memory_layout::phys_to_virt;
    taskdump_raw_line(b"FTX-USTACK sp=");
    taskdump_dec(usp);
    taskdump_raw_line(b"\n");
    let as_arc = match t.address_space_arc() {
        Some(a) => a,
        None => {
            taskdump_raw_line(b"FTX-USTACK no-mm\n");
            return;
        }
    };
    let root = as_arc.root_ppn();
    // Read one user u32 through the task's page tables.
    // SAFETY: diagnostic walk + linear-map read of the task's own mm.
    let read_u32 = |va: u64| -> Option<u32> {
        unsafe {
            let (ppn, _) = PageTableWalker::walk(root, va)?;
            let base = phys_to_virt(crate::arch::mm::memory_layout::PhysAddr(
                (ppn << 12) as u64,
            ));
            Some(core::ptr::read_volatile(
                (base.as_usize() + (va & 0xfff) as usize) as *const u32,
            ))
        }
    };
    let read_u64 = |va: u64| -> Option<u64> {
        unsafe {
            let (ppn, _) = PageTableWalker::walk(root, va)?;
            let base = phys_to_virt(crate::arch::mm::memory_layout::PhysAddr(
                (ppn << 12) as u64,
            ));
            Some(core::ptr::read_volatile(
                (base.as_usize() + (va & 0xfff) as usize) as *const u64,
            ))
        }
    };
    // Xorg input-thread globals: guard ptr @0x1c9a58, input_mutex @0x1c81f0,
    // lock counter @0x206598.
    if let Some(p) = read_u64(0x1c9a58) {
        taskdump_raw_line(b"FTX-GUARDPTR=");
        taskdump_dec(p);
        if let Some(v) = read_u32(p) {
            taskdump_raw_line(b" *p=");
            taskdump_dec(v as u64);
        }
        taskdump_raw_line(b"\n");
    }
    if let Some(v) = read_u32(0x1c81f0) {
        taskdump_raw_line(b"FTX-MUTEXVAL=");
        taskdump_dec(v as u64);
        taskdump_raw_line(b"\n");
    }
    if let Some(v) = read_u32(0x206598) {
        taskdump_raw_line(b"FTX-LOCKCOUNT=");
        taskdump_dec(v as u64);
        taskdump_raw_line(b"\n");
    }
    let lo = usp.saturating_sub(0x200) & !0xfu64;
    let hi = usp + 0x900;
    let mut addr = lo;
    let mut shown = 0;
    while addr < hi && shown < 60 {
        // SAFETY: diagnostic page-table walk of the task's own mm.
        let found = unsafe { PageTableWalker::walk(root, addr) };
        let (ppn, _) = match found {
            Some(x) => x,
            None => {
                addr = (addr + 0x1000) & !0xfffu64;
                continue;
            }
        };
        let page_va = phys_to_virt(crate::arch::mm::memory_layout::PhysAddr(
            (ppn << 12) as u64,
        ));
        let mut off = (addr & 0xfff) as usize;
        while off < 0x1000 && addr < hi && shown < 60 {
            // SAFETY: linear-mapped physical page; read-only diagnostic.
            let v = unsafe {
                core::ptr::read_volatile((page_va.as_usize() + off) as *const u64)
            };
            // User code pointers: Xorg image (< 0x1d0000) or shared libs
            // (0x0000_3000_xxxx_xxxx) or any canonical user text pointer.
            // sc119: OH app children park with library frames at
            // 0x0000_7ff0_xxxx_xxxx (sc118 USEGV rip 0x7ff000fe1b07 family)
            // — same 4-GB-prefix test as the other two families.
            let is_user_ptr = (v > 0x1000 && v < 0x4000_0000_0000)
                && (v < 0x00d0_0000 || (v >> 32) == 0x3000 || (v >> 32) == 0x3fff || (v >> 32) == 0x7ff0);
            if is_user_ptr {
                taskdump_raw_line(b"FTX-US +");
                taskdump_dec(addr.wrapping_sub(usp) as i64 as u64);
                taskdump_raw_line(b" =");
                taskdump_dec(v);
                taskdump_raw_line(b"\n");
                shown += 1;
            }
            addr += 8;
            off += 8;
            if off >= 0x1000 {
                break;
            }
        }
    }
    if shown == 0 {
        taskdump_raw_line(b"FTX-USTACK none\n");
    }
    drop(as_arc);
}

/// Snapshot every task in the PID hash. One line per task, plus a header
/// so the dump is self-describing in a log full of interleaved output.
// ---- dfx-taskdump-bt arch accessors --------------------------------
// The bt lines read the task's last trap frame: (user pc, user sp,
// syscall nr, arg0..arg2). Field names differ per arch: riscv64 keeps
// the RISC-V names (epc/sp/a7/orig_a0..a2), x86_64 keeps
// rip/rsp/orig_rax/rdi/rsi/rdx. nr==0 means "in pure user mode" on
// both (no syscall in flight).
#[cfg(feature = "dfx-taskdump-bt")]
unsafe fn bt_frame(
    pr: *mut crate::arch::pt_regs::PtRegs,
) -> (u64, u64, u64, u64, u64, u64) {
    #[cfg(feature = "riscv64")]
    unsafe { ((*pr).epc, (*pr).sp, (*pr).a7, (*pr).orig_a0, (*pr).a1, (*pr).a2) }
    #[cfg(feature = "x86_64")]
    unsafe { ((*pr).rip, (*pr).rsp, (*pr).orig_rax, (*pr).rdi, (*pr).rsi, (*pr).rdx) }
    #[cfg(not(any(feature = "riscv64", feature = "x86_64")))]
    unsafe { (0, 0, 0, 0, 0, 0) }
}

/// Kernel-text window for the stack-word return-address filter.
/// riscv64: kernel half from 0xffffffff80000000; x86_64: linked image
/// at 0xffffffff80200000 (boot log "image 0xffffffff80200000-...") with
/// headroom to 0xffffffff82000000 for the tail/bss — diagnostics only,
/// a stray extra word costs nothing.
#[cfg(feature = "dfx-taskdump-bt")]
const fn bt_text_window() -> (u64, u64) {
    #[cfg(feature = "riscv64")]
    { (0xffff_ffff_8000_0000, 0xffff_ffff_8020_0000) }
    #[cfg(feature = "x86_64")]
    { (0xffff_ffff_8020_0000, 0xffff_ffff_8200_0000) }
    #[cfg(not(any(feature = "riscv64", feature = "x86_64")))]
    { (0, 0) }
}

/// Plausible-user-pointer window for uepc/usp (matches the historical
/// 0x1000..0x0000_4000_0000_0000 guards).
#[cfg(feature = "dfx-taskdump-bt")]
const fn bt_user_window() -> (u64, u64) {
    (0x1000, 0x0000_4000_0000_0000)
}

pub fn dump_all_tasks(reason: &str) {
    puts("\n=== DFX TASK DUMP (");
    puts(reason);
    puts(") ===\n");
    // R57-scribble: one-time Task-field offset map — the pk= garbage
    // captures (ti_pick_state reading 91/112/158/122400/0x38E0000 on
    // live tasks) prove a writer lands inside the thread_info
    // neighborhood; the exact offsets identify which 8-byte field's
    // low/high half overlaps the pick-state word.
    {
        static OFFS_PRINTED: core::sync::atomic::AtomicBool =
            core::sync::atomic::AtomicBool::new(false);
        if !OFFS_PRINTED.swap(true, core::sync::atomic::Ordering::AcqRel) {
            use crate::process::task::task_offsets as to;
            puts("taskoff:");
            let offs: [(&str, usize); 11] = [
                (" ti_flags", to::TI_FLAGS),
                (" ti_kernel_sp", to::TI_KERNEL_SP),
                (" ti_user_sp", to::TI_USER_SP),
                (" ti_cpu", to::TI_CPU),
                (" cpus_allowed", to::TI_CPUS_ALLOWED),
                (" hung_task_since", to::TI_HUNG_TASK_SINCE),
                (" task_refcnt", to::TI_TASK_REFCNT),
                (" journal_handle", to::TI_JOURNAL_HANDLE),
                (" ti_on_cpu", to::TI_ON_CPU),
                (" running_on_cpu", to::TI_RUNNING_ON_CPU),
                (" ti_pick_state", to::TI_PICK_STATE),
            ];
            for (name, off) in offs {
                puts(name);
                puts("=0x");
                put_hex(off as u64);
            }
            putc(b'\n');
        }
    }
    // R56-diag: per-CPU slot currents (who each slot thinks is running).
    {
        let cs = crate::sched::sched::grq_diag_cpu_currents();
        puts("cpu slots:");
        for c in 0..crate::config::MAX_CPUS {
            putc(b' ');
            let cur = cs[c].2;
            if cur.is_null() { put_dec(999999); } else { put_dec(unsafe { (*cur).pid() as u64 }); }
        }
        putc(b'\n');
    }

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
                    let (epc, _usp, _nr, _a0, _a1, _a2) = unsafe { bt_frame(pr) };
                    let (lo, hi) = bt_user_window();
                    if epc > lo && epc < hi {
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
                        if v >= bt_text_window().0 && v < bt_text_window().1 && (v & 1) == 0 {
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
    let mut linked_unknown: usize = 0;
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
            // Thread-group attribution: comm alone cannot say which
            // process an OS_FFRT_2_2 or OS_IPC_3 worker serves, and the
            // b140 hunt needed exactly that (accountmgr's FFRT worker
            // normal-exited mid-activation while its IPC threads kept
            // answering callbacks). tgid= names the owner unambiguously.
            puts(" tgid=");
            put_dec(t.tgid() as u64);
            puts(" state=");
            puts(state_name(t.state().bits()));
            puts(" ti_cpu=");
            put_dec(t.ti_cpu() as u64);
            puts(" on_cpu=");
            put_dec(t.on_cpu() as u64);
            // GNOME-wedge hunt: the CFS pick predicate refuses a task when
            // its running_on_cpu claim names another CPU, so a leaked claim
            // strands a linked task permanently (g2: 10 tasks linked, all
            // CPUs idle-looping, nothing picked for 35 min). The claim was
            // the ONE ownership field the dump never printed — add it.
            {
                let claim = t.running_on_cpu.load(core::sync::atomic::Ordering::Relaxed);
                puts(" claim=");
                if (claim as i64) < 0 {
                    putc(b'-');
                    put_dec((claim as i64).unsigned_abs());
                } else {
                    put_dec(claim as u64);
                }
            }
            // R57: the pick-state word (0=PICKED 1=LIVE 2=REVOKED) is what
            // arbitrates the slot-capture steal — printed beside the claim
            // it settles "in-flight pick vs live runner vs already stolen"
            // in one dump line (the n1-organic panic hinged on exactly
            // this distinction).
            puts(" pk=");
            let pkv = t.pick_state();
            put_dec(pkv as u64);
            if pkv > 2 {
                // Scribbler fingerprint: any other value is a foreign store
                // landing on this word. Make it loud and unmistakable.
                puts(" PK-GARBAGE");
            }
            puts(" preempt=");
            let pv = t.preempt_count() as u64;
            if (pv as i64) < 0 {
                putc(b'-');
                put_dec((pv as i64).unsigned_abs());
            } else {
                put_dec(pv);
            }
            // Authoritative linked-state (tree/list scan, not the flag) —
            // settles flag-desync vs really-off-queue in one shot. The scan
            // runs under the GRQ try-lock; a busy lock reports '?' (counted
            // below) — never a lockless tree walk, which races structural
            // mutation on other CPUs and corrupts the iterator.
            let linked = crate::sched::sched::grq_diag_cfs_linked(task_ptr);
            match linked {
                Some(b) => {
                    puts(" linked=");
                    put_dec(b as u64);
                }
                None => {
                    puts(" linked=?");
                    linked_unknown += 1;
                }
            }
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
            // Last syscall latched at dispatch ENTRY (per-task): for a
            // sleeping task this names the syscall it is parked in —
            // the discriminator for "which kernel object is this waiter
            // on" hunts (account-activation stall: epoll? futex? binder
            // ioctl? signalfd?).
            {
                let nr = t.last_syscall_nr.load(core::sync::atomic::Ordering::Relaxed);
                if nr != 0 {
                    let a0 = t.last_syscall_a0.load(core::sync::atomic::Ordering::Relaxed);
                    let a1 = t.last_syscall_a1.load(core::sync::atomic::Ordering::Relaxed);
                    puts(" lastsys=");
                    put_dec(nr as u64);
                    puts("(");
                    put_hex(a0);
                    putc(b',');
                    put_hex(a1);
                    puts(")");
                } else {
                    puts(" lastsys=?");
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
                        let (epc, usp, a7, a0, a1, a2) = unsafe { bt_frame(pr) };
                        let (ulo, uhi) = bt_user_window();
                        if epc > ulo && epc < uhi {
                            puts(" uepc=");
                            put_hex(epc);
                        }
                        if usp > ulo && usp < uhi {
                            puts(" usp=");
                            put_hex(usp);
                        }
                        // Last syscall args (nr, arg0..arg2) — identifies
                        // the futex word a sleeper is parked on. riscv64
                        // keeps the nr in a7 (98=futex); x86_64 in
                        // orig_rax (202=futex).
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
                        // Futex sleeper: dump the word + waiter chain truth.
                        // These helpers are defined under `dfx-futex-trace`
                        // (they print through the FTX tracer) — the call
                        // sites must carry the SAME gate or the
                        // `dfx-taskdump-bt` feature fails to link alone.
                        #[cfg(feature = "dfx-futex-trace")]
                        {
                            #[cfg(feature = "riscv64")]
                            const FUTEX_NR: u64 = 98;
                            #[cfg(feature = "x86_64")]
                            const FUTEX_NR: u64 = 202;
                            #[cfg(not(any(feature = "riscv64", feature = "x86_64")))]
                            const FUTEX_NR: u64 = 0;
                            if a7 == FUTEX_NR && a0 > 0x1000 && (a0 & 0x3) == 0 {
                                if t.state().is_sleeping() {
                                    // sc119: the OH app-child hunt. Hundreds of
                                    // futex sleepers exist per dump; the word
                                    // read inside FTX-STATE lands in the
                                    // DUMPER's mm anyway (wrong address space)
                                    // and a full ustack per sleeper floods the
                                    // UART. Gate the expensive dumps to the
                                    // hunt page 0x7fffffffc000 (every process's
                                    // stack-top-minus-3rd page, where all 13
                                    // app children parked); the bucket-chain
                                    // walk inside FTX-STATE is kernel-global
                                    // and stays valid there.
                                    if (a0 & !0xfffu64) == 0x7fffffffc000 {
                                        crate::sync::futex::dfx_dump_futex_state(
                                            a0 as usize,
                                            t.pid(),
                                        );
                                        dfx_dump_user_stack(&t, usp);
                                    }
                                }
                            }
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
                        if v >= bt_text_window().0 && v < bt_text_window().1 && (v & 1) == 0 {
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
    if linked_unknown > 0 {
        puts("warning: ");
        put_dec(linked_unknown as u64);
        puts(" linked= probes skipped (GRQ lock busy)\n");
    }
    // Heap/page snapshot on every periodic dump: the leak curve rides the
    // same 5s cadence, and with dfx=memwatch the size-class growth table
    // and big-object call-site histogram follow (memwatch.rs). Only from
    // the periodic path — the watchdog variant may run while the heap lock
    // is the wedged one, and the stats walk takes allocator locks.
    if reason == "periodic" {
        // R85 (b148 crawl hunt): the ext4 biglock's own state beside the
        // task snapshot — the b148 shape (whole OH cohort D-parked, lock
        // apparently free, zero STALL prints) was invisible without it.
        crate::fs::ext4::EXT4_BIG_LOCK.dfx_census_line();
        crate::dfx::memwatch::dump_mem();
        // pa16 (magazine): the packing layer's per-class op counts and
        // class-lock acquisitions on the same cadence — locks vs
        // obj_allocs+obj_frees is the magazine's live traffic-ratio
        // readout (atomic loads + raw UART only, no allocator lock,
        // safe from the periodic path).
        crate::mm::small_alloc::census_line();
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

// ============================================================================
// rcu-gp-frozen monopolist truth dump (52nd session, oh-crash lane)
// ============================================================================

/// Kernel-text window for the live stack scan (same bounds as
/// bt_text_window's x86_64 arm, kept independent of the
/// dfx-taskdump-bt feature so this probe builds in the stock kernel).
#[cfg(feature = "x86_64")]
const FROZEN_TEXT_LO: u64 = 0xffff_ffff_8020_0000;
#[cfg(feature = "x86_64")]
const FROZEN_TEXT_HI: u64 = 0xffff_ffff_8200_0000;

/// Answer block filled by the IPI callback ON the frozen CPU. Static so
/// the CSD callback (IRQ context) never touches the caller's stack.
#[cfg(feature = "x86_64")]
struct FrozenAnswer {
    answered: core::sync::atomic::AtomicBool,
    gs_cpu: core::sync::atomic::AtomicU32,
    gs_magic: core::sync::atomic::AtomicU64,
    lapic: core::sync::atomic::AtomicU32,
    int_rip: core::sync::atomic::AtomicU64,
    int_rsp: core::sync::atomic::AtomicU64,
    stack: [core::sync::atomic::AtomicU64; 12],
    stack_n: core::sync::atomic::AtomicUsize,
}

#[cfg(feature = "x86_64")]
static FROZEN_ANSWER: FrozenAnswer = FrozenAnswer {
    answered: core::sync::atomic::AtomicBool::new(false),
    gs_cpu: core::sync::atomic::AtomicU32::new(u32::MAX),
    gs_magic: core::sync::atomic::AtomicU64::new(0),
    lapic: core::sync::atomic::AtomicU32::new(u32::MAX),
    int_rip: core::sync::atomic::AtomicU64::new(0),
    int_rsp: core::sync::atomic::AtomicU64::new(0),
    stack: [const { core::sync::atomic::AtomicU64::new(0) }; 12],
    stack_n: core::sync::atomic::AtomicUsize::new(0),
};

/// CSD callback: runs ON the frozen CPU, in CALL_FUNCTION IPI context.
/// Reads ONLY per-CPU/own-stack state — no locks the monopolist could
/// hold (CURRENT_PT_REGS is an atomic slot; the LAPIC id is an MSR/IO
/// read; the stack scan reads the interrupted stack downward from the
/// IPI frame — all mapped while the task runs).
#[cfg(feature = "x86_64")]
fn frozen_ipi_probe(_arg: *mut core::ffi::c_void) {
    use core::sync::atomic::Ordering;
    let a = &FROZEN_ANSWER;

    // (a) GS-derived cpu id + the GS-base magic word — the percpu
    // misdirection canary (51st session's prime suspect: cpu1's
    // cpu_id() resolving to 0 would land its QS reports in slot 0).
    let gs_cpu = crate::arch::x86_64::smp::gs_cpu_id() as u32;
    let gs_magic_q: u64;
    // SAFETY: %gs:PC_MAGIC reads this CPU's PerCpu.magic — a plain data
    // read of the active GS base, exactly what the trap stubs do.
    unsafe {
        core::arch::asm!(
            "mov {0}, qword ptr gs:[{1}]",
            out(reg) gs_magic_q,
            const crate::arch::x86_64::smp::PC_MAGIC,
            options(nomem, nostack, preserves_flags)
        );
    }
    a.gs_cpu.store(gs_cpu, Ordering::Relaxed);
    a.gs_magic.store(gs_magic_q, Ordering::Relaxed);

    // (b) LAPIC id — hardware CPU identity, completely independent of
    // the GS base. gs_cpu vs lapic mismatch convicts misdirection.
    a.lapic.store(crate::drivers::intc::apic::this_lapic_id(), Ordering::Relaxed);

    // (c) The interrupted frame: CURRENT_PT_REGS[cpu] was swapped to
    // THIS IPI's PtRegs by trap_handler before dispatch, so its rip/rsp
    // are the monopolist loop's live instruction and stack pointer.
    let pr = crate::arch::trap::current_pt_regs() as *const crate::arch::pt_regs::PtRegs;
    if !pr.is_null() && (pr as u64) >= 0xffff_8000_0000_0000 {
        // SAFETY: diagnostic read of the live IPI frame's fixed slots.
        let (rip, rsp) = unsafe { ((*pr).rip, (*pr).rsp) };
        a.int_rip.store(rip, Ordering::Relaxed);
        a.int_rsp.store(rsp, Ordering::Relaxed);

        // (d) Live stack scan: from the interrupted RSP upward (toward
        // the frame's callers), keep kernel-text return addresses.
        if rsp > 0x1000 && (rsp & 7) == 0 {
            let mut n = 0usize;
            for k in 0..64 {
                // SAFETY: diagnostic read inside the running task's own
                // kernel stack, which stays mapped while it monopolizes
                // the CPU (worst case: a word beyond the live frames).
                let v = unsafe {
                    core::ptr::read_volatile((rsp + k * 8) as *const u64)
                };
                if v >= FROZEN_TEXT_LO && v < FROZEN_TEXT_HI && (v & 1) == 0 {
                    a.stack[n].store(v, Ordering::Relaxed);
                    n += 1;
                    if n >= a.stack.len() {
                        break;
                    }
                }
            }
            a.stack_n.store(n, Ordering::Relaxed);
        }
    }
    a.answered.store(true, Ordering::Release);
}

/// One-shot cross-CPU truth dump for the frozen-CPU monopolist hunt.
/// Called from rcu.rs's grace-freeze one-shot on the WAITING cpu:
/// 1. prints every CPU's timer-tick count + last tick-entry RIP +
///    tick-entry preempt count (a stalled tick count on the frozen CPU
///    = IRQs off; an advancing count + frozen QS = a preempt-loop with
///    IRQs on — and its last tick-entry RIP names the loop directly);
/// 2. IPIs the stuck CPU with [`frozen_ipi_probe`], which answers with
///    the percpu canary (gs cpu id + magic vs LAPIC id) and the
///    interrupted RIP + live stack — or stays silent, which itself
///    convicts an IRQ-off monopolist.
/// Anomaly-gated one-shot per boot (caller latches); zero cost on a
/// healthy boot.
pub fn dump_frozen_cpu_truth(stuck: usize) {
    #[cfg(feature = "x86_64")]
    {
        puts("\n=== FROZEN-CPU TRUTH (stuck=");
        put_dec(stuck as u64);
        puts(") ===\n");
        for c in 0..crate::config::MAX_CPUS {
            if !crate::sched::sched::cpu_online(c) {
                continue;
            }
            puts("FROZEN-CPU ticks cpu");
            put_dec(c as u64);
            puts(": n=");
            put_dec(crate::fs::procfs::interrupts::timer_count(c));
            puts(" last_rip=");
            put_hex(
                crate::interrupt::preempt::TICK_ENTRY_RIP[c]
                    .load(core::sync::atomic::Ordering::Relaxed),
            );
            puts(" entry_pc=");
            put_hex(
                crate::interrupt::preempt::TICK_ENTRY_PC[c]
                    .load(core::sync::atomic::Ordering::Relaxed) as u64,
            );
            putc(b'\n');
        }

        if stuck >= crate::config::MAX_CPUS
            || !crate::sched::sched::cpu_online(stuck)
        {
            return;
        }

        FROZEN_ANSWER.answered.store(false, core::sync::atomic::Ordering::Release);
        let ok = crate::arch::x86_64::ipi::smp_call_function_timeout(
            stuck,
            frozen_ipi_probe,
            core::ptr::null_mut(),
            200,
        );
        if !ok {
            puts("FROZEN-CPU ipi: cpu");
            put_dec(stuck as u64);
            puts(" DID NOT ANSWER in 200ms — IRQs off or CSD-queue wedge (tick line above disambiguates)\n");
            // IF=0 fallback: the stuck CPU's current task (sched slot) is
            // the victim; its LIVE frames sit BELOW the sp saved at its
            // last switch-in (t.thread().sp — the restore point; the task
            // has only called deeper since). Scan downward and print
            // kernel-text words: names the spin loop and, via the frames
            // above it, the lock's owner path. Read-side only, from this
            // CPU; races cost at worst a torn-but-bounded word.
            let cur = crate::sched::sched::grq_diag_cpu_currents();
            let t = cur[stuck].2 as *mut crate::process::task::Task;
            if !t.is_null() {
                // SAFETY: diagnostic read of stable fields on the
                // per-CPU current Task (idle/static or heap-allocated;
                // both stay valid while current).
                let sp = unsafe { (*t).thread().sp };
                puts("FROZEN-CPU victim pid=");
                put_dec(unsafe { (*t).pid() } as u64);
                puts(" preempt=");
                put_dec(unsafe { (*t).preempt_count() } as u64);
                puts(" saved_sp=");
                put_hex(sp);
                if (sp >> 48) == 0xffff && (sp & 7) == 0 && sp > 0x1000 {
                    puts(" stack:");
                    let base = sp as *const u64;
                    let mut shown = 0usize;
                    for k in 0..96 {
                        // SAFETY: diagnostic read within the victim's own
                        // kernel stack pages (mapped while the Task exists).
                        let v = unsafe { core::ptr::read_volatile(base.wrapping_sub(k)) };
                        if v >= FROZEN_TEXT_LO && v < FROZEN_TEXT_HI && (v & 1) == 0 {
                            putc(b' ');
                            put_hex(v);
                            shown += 1;
                            if shown >= 16 {
                                break;
                            }
                        }
                    }
                }
                putc(b'\n');
            }
            return;
        }
        use core::sync::atomic::Ordering;
        let a = &FROZEN_ANSWER;
        if !a.answered.load(Ordering::Acquire) {
            // Defensive: completion without the answer flag is not
            // reachable (callback sets both), but never wedge the dump.
            return;
        }
        puts("FROZEN-CPU ipi cpu");
        put_dec(stuck as u64);
        puts(": gs_cpu=");
        put_dec(a.gs_cpu.load(Ordering::Relaxed) as u64);
        puts(" gs_magic=");
        put_hex(a.gs_magic.load(Ordering::Relaxed));
        puts(" lapic=");
        put_hex(a.lapic.load(Ordering::Relaxed) as u64);
        puts(" int_rip=");
        put_hex(a.int_rip.load(Ordering::Relaxed));
        puts(" int_rsp=");
        put_hex(a.int_rsp.load(Ordering::Relaxed));
        let n = a.stack_n.load(Ordering::Relaxed);
        puts(" stack:");
        for k in 0..n.min(a.stack.len()) {
            putc(b' ');
            put_hex(a.stack[k].load(Ordering::Relaxed));
        }
        putc(b'\n');
    }
    #[cfg(not(feature = "x86_64"))]
    {
        let _ = stuck;
    }
}

/// 53rd session (52nd §6a): parked-kstack symbolizer for S-state tasks.
///
/// A task sleeping in the kernel parks with its full call chain intact
/// between its saved switch sp (`t.thread().sp`, the restore point of
/// the last context switch, set inside schedule()) and the entry frame
/// at the stack top. Walking UPWARD from the saved sp and symbolizing
/// every kernel-text word names the innermost wait function — wait
/// queue, completion, SleepMutex, bio page wait — which the syscall
/// number alone cannot (hilogd main parked `lastsys=9` for 600 s with
/// no way to see what inside the kernel it sleeps on).
///
/// Dedup: a (pid, saved-sp) pair prints its line once; a parked task's
/// stack is frozen while it sleeps, so repeats carry zero information.
/// Bounded ring of 256 entries; allocation-free (raw atoms).
///
/// SAFETY contract: the caller holds no pid-hash bucket lock while the
/// RETURNED STRING is printed (sc207); the walk itself only
/// read_volatile's words inside the sleeping task's own kernel-stack
/// pages, which the Task pins. A task that wakes mid-walk costs a torn
/// word at worst.
pub fn parked_kstack_syms(t: &crate::process::task::Task) -> alloc::string::String {
    use core::fmt::Write as _;
    use crate::process::task::Task;

    // Kernel text window (same bounds discipline as the R76 HUNG-STK
    // scan; the two arch linker scripts spell the bounds differently).
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
    let lo = core::ptr::addr_of!(__stext) as u64;
    #[cfg(feature = "x86_64")]
    let hi = core::ptr::addr_of!(__etext) as u64;
    #[cfg(not(feature = "x86_64"))]
    let lo = core::ptr::addr_of!(_stext) as u64;
    #[cfg(not(feature = "x86_64"))]
    let hi = core::ptr::addr_of!(_etext) as u64;

    let mut s = alloc::string::String::new();
    let thsp = t.thread().sp;
    // Guard: canonical kernel stack pointer, 8-aligned, inside the
    // task's own stack window (checked against kernel_stack_bottom so
    // the upward walk cannot leave the task's pages).
    let bottom = t.kernel_stack_bottom();
    if (thsp >> 48) != 0xffff || (thsp & 7) != 0 || thsp < 0x1000 {
        return s;
    }
    if bottom != 0
        && (thsp < bottom as u64
            || thsp >= (bottom + crate::config::KERNEL_STACK_SIZE) as u64)
    {
        return s;
    }
    // Dedup ring: (pid, sp) already printed -> skip everything.
    {
        const N: usize = 256;
        static SEEN_PID: [core::sync::atomic::AtomicU32; N] =
            [const { core::sync::atomic::AtomicU32::new(0) }; N];
        static SEEN_SP: [core::sync::atomic::AtomicU64; N] =
            [const { core::sync::atomic::AtomicU64::new(0) }; N];
        static CUR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
        let pid = t.pid() as u32;
        for i in 0..N {
            if SEEN_PID[i].load(Ordering::Relaxed) == pid
                && SEEN_SP[i].load(Ordering::Relaxed) == thsp
            {
                return s; // same park, already reported
            }
        }
        let idx = CUR.fetch_add(1, Ordering::Relaxed) % N;
        SEEN_PID[idx].store(pid, Ordering::Relaxed);
        SEEN_SP[idx].store(thsp, Ordering::Relaxed);
    }
    let base = thsp as *const u64;
    let mut shown = 0usize;
    // Segment discipline: printk records cap at 256 bytes, and the
    // sleepsnap emitter chunks on newline boundaries — so emit the walk
    // as newline-separated segments of at most ~230 payload bytes each
    // (header carries tid for reassembly). ohc25's unsegmented lines
    // all died at the record cap mid-symbol.
    let mut seg = alloc::string::String::new();
    let mut seg_len = 0usize;
    let _ = write!(seg, "parkstk: tid {} sp={:#x}:", t.pid(), thsp);
    seg_len = seg.len();
    let mut flush = |seg: &mut alloc::string::String, s: &mut alloc::string::String| {
        seg.push('\n');
        s.push_str(seg);
        seg.clear();
    };
    // Walk upward: live caller frames of the parked chain, bounded by
    // the task's own stack top.
    // ohc27 lesson: kernel stacks are 128 KiB (config::KERNEL_STACK_SIZE)
    // and the reference-parity wait site can sit many KiB above the saved
    // switch sp — a 256-word (2 KiB) window found only the switch
    // machinery. Walk to the stack top (up to 4096 words = 32 KiB).
    let limit = if bottom != 0 {
        (((bottom + crate::config::KERNEL_STACK_SIZE) as u64 - thsp) / 8).min(4096) as usize
    } else {
        4096
    };
    for k in 0..limit {
        // SAFETY: diagnostic read inside the sleeping task's own kernel
        // stack pages (pinned by the Task while it exists).
        let v = unsafe { core::ptr::read_volatile(base.add(k)) };
        if v >= lo && v < hi && (v & 1) == 0 {
            let sym = crate::dfx::ksyms::format_symbol(v);
            if seg_len + sym.len() + 1 > 230 {
                flush(&mut seg, &mut s);
                let _ = write!(seg, "parkstk+: tid {}:", t.pid());
                seg_len = seg.len();
            }
            let _ = write!(seg, " {}", sym);
            seg_len += sym.len() + 1;
            shown += 1;
            if shown >= 16 {
                break;
            }
        }
    }
    if shown > 0 {
        flush(&mut seg, &mut s);
    }
    s
}

// ---------------------------------------------------------------------------
// Deferred census emission (54th): the S-state census collects under the
// timer softirq, where a printk emitter carries a 100us console-emit
// budget while the OH hilog flood holds the console ~0.4-17ms per line —
// ohc36 lost ~30% of the uvma map lines (and other CPUs' /dev/console
// writes spliced mid-census-record). Producers push pre-chunked LINES
// (each already under the printk record cap) here; a task-context
// kthread drains and printk()s them — task context waits up to 2s for
// the console and always gets through (the mmprofd discipline, lb332).
// Drop-oldest when full: census lines are diagnostic, a stuck boot
// regenerates them every snapshot.
// ---------------------------------------------------------------------------

const DEFER_LINE_MAX: usize = 256;

struct DeferredEmit {
    ring: [Option<alloc::string::String>; DEFER_LINE_MAX],
    head: usize,
    tail: usize,
    dropped: u64,
}

static DEFER_Q: crate::sync::spinlock::Spinlock<DeferredEmit> =
    crate::sync::spinlock::Spinlock::new(DeferredEmit {
        ring: [const { None }; DEFER_LINE_MAX],
        head: 0,
        tail: 0,
        dropped: 0,
    });

/// Queue one census line for task-context emission. Producers may be
/// softirq (timer heartbeat census). The line must already be under the
/// printk record text cap (240 bytes) — the sleep_snapshot chunker
/// guarantees that for everything it feeds here.
pub fn defer_emit_line(line: alloc::string::String) {
    if line.is_empty() {
        return;
    }
    {
        let mut q = DEFER_Q.lock_irqsave();
        let next = (q.tail + 1) % DEFER_LINE_MAX;
        if next == q.head {
            // Full: drop the OLDEST line.
            let h = q.head;
            q.ring[h] = None;
            q.head = (h + 1) % DEFER_LINE_MAX;
            q.dropped += 1;
        }
        let t = q.tail;
        q.ring[t] = Some(line);
        q.tail = next;
    }
    ensure_defer_thread();
}

/// Spawn (once) the task-context consumer kthread for `defer_emit_line`.
pub fn ensure_defer_thread() {
    static THREAD_ON: core::sync::atomic::AtomicBool =
        core::sync::atomic::AtomicBool::new(false);
    if THREAD_ON.swap(true, core::sync::atomic::Ordering::AcqRel) {
        return;
    }
    crate::process::kthread::kthread_run(
        defer_emit_fn,
        core::ptr::null_mut(),
        "censusd",
    );
}

extern "C" fn defer_emit_fn(_arg: *mut core::ffi::c_void) -> i32 {
    use crate::process::task::{Task, TaskState};
    loop {
        if crate::process::kthread::kthread_should_stop() {
            break;
        }
        // Drain everything queued first.
        loop {
            let line = {
                let mut q = DEFER_Q.lock_irqsave();
                if q.head == q.tail {
                    if q.dropped > 0 {
                        let d = q.dropped;
                        q.dropped = 0;
                        Some(alloc::format!(
                            "censusd: {} deferred lines dropped (queue full)",
                            d
                        ))
                    } else {
                        None
                    }
                } else {
                    let h = q.head;
                    let l = q.ring[h].take();
                    q.head = (h + 1) % DEFER_LINE_MAX;
                    l
                }
            };
            match line {
                Some(l) => crate::printk::printk_ln(
                    crate::printk::loglevel::KERN_INFO,
                    format_args!("{}", l),
                ),
                None => break,
            }
        }
        // Sleep 250 ms (jiffies discipline as mmprofd).
        let current = match crate::sched::current() {
            Some(t) => t as *mut Task,
            None => return 0,
        };
        // SAFETY: current is the running task's pointer (we are it).
        let pid = unsafe { (*current).pid() };
        let wake_at = crate::drivers::timer::get_jiffies() + 25;
        let timer_id = crate::timer::add_timer_wakeup(wake_at, pid);
        // SAFETY: setting our own task's state.
        unsafe {
            (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
        }
        crate::arch::cpu::restore_irq(true);
        crate::sched::schedule();
        if timer_id != 0 {
            crate::timer::del_timer(timer_id);
        }
    }
    0
}

/// Early-cadence wrapper for the S-state sleep snapshot (53rd): the
/// khungtaskd cadence (60 s) sees a 7.4 s park only at 61 s; this runs
/// the same snapshot every ~5 s for the first ~60 s of boot so the
/// park-time kernel stack is captured while the boot is still young.
/// Self-gated on dfx=sleepsnap inside sleep_snapshot itself.
pub fn sleep_snapshot_early() {
    use core::sync::atomic::Ordering;
    static LAST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    static N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    let now = crate::drivers::timer::get_jiffies() as u64;
    let last = LAST.load(Ordering::Relaxed);
    if now.saturating_sub(last) < 500 || N.load(Ordering::Relaxed) >= 12 {
        return;
    }
    if LAST
        .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
        .is_err()
    {
        return;
    }
    N.fetch_add(1, Ordering::Relaxed);
    crate::ipc::binder::sleep_snapshot();
    // 53rd drain hunt: park-side AF_UNIX queue/wait census on the same
    // cadence (self-gated on dfx=sleepsnap inside).
    crate::net::unix::unix_park_audit();
    // 53rd timer-drive hunt: armed timerfd wheel entries + waiter census
    // (self-gated on dfx=sleepsnap inside).
    crate::syscall::misc::timerfd_wheel_audit();
}
