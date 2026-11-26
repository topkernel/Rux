//! The x86_64 trap-path forensic/report halves (arch-cold lane):
//! everything here MOVED from kernel/core/src/arch/x86_64/trap.rs — the
//! fatal-fault death reports (fatal user #GP/#PF: the register/VMA/
//! stack-word decode, the failing-syscall tail, the per-tgid mm-op
//! rings), the cross-stack scribble foreign-entry report, the
//! segv-history ring, and the ripsample/stime report formatters.
//!
//! The trap handler bodies, the tick-path samplers (utime/stime_
//! sample_rip, comm_has_launcher) and the panic printers stayed
//! framework-side (hot sites call them by name; the KERNPANIC path
//! must print before any table is published). The framework keeps
//! same-name wrappers dispatching the ArchDiagHooks rows.
//!
//! Raw reads cross through the audited framework edges:
//! `crate::arch::x86_64::unsafe::diag_*` and
//! `crate::arch::x86_64::trap::diag_tss_rsp0` — read-only diagnostic
//! bottoms (INV-ARCHDIAG rows in the framework ledger).

use crate::arch::x86_64::pt_regs::PtRegs;

pub fn gp_regmem_dump(regs: &PtRegs) {
    use core::fmt::Write;
    let Some(tk) = crate::sched::hotpath::current() else { return };
    let Some(aspace) = tk.address_space() else { return };
    let root = aspace.root_ppn();
    let ptrs: [(&str, u64); 8] = [
        ("rax", regs.rax),
        ("rbx", regs.rbx),
        ("rcx", regs.rcx),
        ("rdx", regs.rdx),
        ("rsi", regs.rsi),
        ("rdi", regs.rdi),
        ("r8", regs.r8),
        ("r9", regs.r9),
    ];
    for (name, v) in ptrs {
        // hp88 hunt: a corrupted heap pointer inside a live perf ring's
        // mapping is the placement-overlap conviction.
        if let Some((va, len, id)) = crate::perf::ring_covers(v) {
            crate::pr_debug!(
                            "GP-REGMEM: {}={:#x} INSIDE PERF RING va={:#x} len={:#x} of event {}",
                name, v, va, len, id
            );
        }
        // hp88 hunt: name the victim's PTE + frame + descriptor state —
        // "present + zero content" is either a stray-unmap-then-refault
        // (fresh zero frame) or an in-place zeroed frame; the frame's
        // descriptor and ring membership separate them.
        if v > 0x1_0000 && v < 0x0000_8000_0000_0000 {
            // Read-only page-table walk + frame-descriptor snapshot on a
            // live root (the audited diag edges carry the unsafe).
            if let Some((ppn, bits)) =
                crate::arch::x86_64::r#unsafe::diag_user_pte(root, v & !0xfff)
            {
                let frame = (ppn as usize) << 12;
                let (rc, mc, fl, idx) =
                    crate::arch::x86_64::r#unsafe::diag_frame_desc(ppn);
                let inring = crate::perf::ring_frame_member(ppn as u64);
                crate::pr_debug!(
                            "GP-PTE: {}={:#x} pte={:#x} frame={:#x} rc={} mapcount={} ring_frame={} flags={:#x} rmapidx={:#x}(va {:#x})",
                    name, v, bits, frame, rc, mc, inring, fl, idx,
                    idx.wrapping_mul(0x1000)
                );
            }
        }
        if v > 0x1_0000 && v < 0x0000_8000_0000_0000 {
            // Checked read returns None on unmapped holes (same shape as
            // user_segv_forensics' nested fore_read_u64 — the audited
            // diag edge carries the walk + volatile read).
            let read_at =
                |va: u64| crate::arch::x86_64::r#unsafe::diag_fore_read_u64(root, va);
            let w0 = read_at(v & !0x7);
            let w1 = read_at((v & !0x7) + 8);
            if w0.is_some() {
                let mut line = alloc::string::String::new();
                let _ = write!(line, "GP-REGMEM: {}={:#x}:", name, v);
                for w in [w0, w1] {
                    match w {
                        Some(x) => {
                            let _ = write!(line, " {:#x}", x);
                        }
                        None => {
                            let _ = write!(line, " ????????????????");
                        }
                    }
                }
                // HOTPATH-PRINT: fatal-path forensics — once per #GP death report
                crate::pr_err!("{}", line);
            }
        }
    }
}

/// hp89 hunt: the victim tgid's mm-syscall history at user-#GP time.
/// The mallocng get_meta corruption needs a live PTE cleared (and its
/// frame freed) or a live frame zeroed in place; whoever did it is
/// either a syscall of the victim's own process (munmap/brk/mremap/
/// madvise — the per-task mm_ops ring names it with the range) or a
/// non-syscall path (fork-child exit teardown, reclaim, migration —
/// the ABSENCE of a covering syscall convicts those). This dump runs
/// AFTER the race window (post-hoc — it cannot perturb it), bounded to
/// the first 8 user #GPs per boot.
pub fn gp_mmhistory_dump(regs: &PtRegs) {
    use core::sync::atomic::Ordering::Relaxed;
    static GPMM_N: core::sync::atomic::AtomicUsize =
        core::sync::atomic::AtomicUsize::new(0);
    if GPMM_N.fetch_add(1, Relaxed) >= 8 {
        return;
    }
    let Some(tk) = crate::sched::hotpath::current() else { return };
    let dump = |tag: &str, t: &crate::process::task::Task| {
        const N: usize = 16;
        let cur = t.ext.mm_ops_cur.load(Relaxed);
        let pid = t.pid();
        let mut shown = 0usize;
        for k in 0..N {
            let i = (cur + N - 1 - k) % N;
            let w0 = t.ext.mm_ops[i * 2].load(Relaxed);
            let w1 = t.ext.mm_ops[i * 2 + 1].load(Relaxed);
            let op = w0 & 0xff;
            if op == 0 {
                continue;
            }
            let rc = (w0 >> 8) & 0xff;
            let addr = w0 >> 16;
            let len = w1 & 0x0000_ffff_ffff_ffff;
            let name = match op {
                1 => "mmap",
                2 => "mprotect",
                3 => "munmap",
                4 => "brk",
                _ => "mremap",
            };
            if shown < 12 {
                crate::pr_debug!(
                            "GP-MMH: [{tag} pid={pid}] {name} addr={addr:#x} len={len:#x} rc={rc}"
                );
            }
            shown += 1;
        }
        if shown == 0 {
            crate::pr_debug!(
                            "GP-MMH: [{tag} pid={pid}] (no mm syscalls recorded)");
        } else if shown > 12 {
            crate::pr_debug!(
                            "GP-MMH: [{tag} pid={pid}] ... {shown}/16 events (capped)");
        }
    };
    dump("self", tk);
    let tgid = tk.tgid();
    let self_ptr = tk as *const crate::process::task::Task;
    let mut sibs = 0usize;
    let _skipped = crate::sched::hotpath::for_each_task_try(|t| {
        if t.tgid() == tgid && (t as *const crate::process::task::Task) != self_ptr {
            sibs += 1;
            if sibs <= 6 {
                dump("sib", t);
            }
        }
    });
    if sibs > 6 {
        crate::pr_debug!(
                            "GP-MMH: [sib] ... 6/{sibs} siblings shown");
    }
    // hp89 hunt: the global PTE-clearing ring tail — the non-syscall
    // clearers (unmap engine calls, dying-mm teardowns, rmap walkers)
    // around the corruption window, plus whether any recorded
    // unmap_pages range covers the corrupted pointer VAs.
    crate::pr_debug!(
                            "GP-UNWATCH: tail of PTE-clearing ring (newest first):");
    crate::mm::unwatch::dump_tail(120);
    for (name, v) in [
        ("rbx", regs.rbx),
        ("rsi", regs.rsi),
        ("rdi", regs.rdi),
        ("rdx", regs.rdx),
    ] {
        if v > 0x1_0000 && v < 0x0000_8000_0000_0000 {
            let hit = crate::mm::unwatch::covered_by_unmap(v & !0xfff);
            crate::pr_debug!(
                            "GP-UNWATCH: {}={:#x} covered_by_unmap={}", name, v, hit);
        }
    }
}

// huks-wedge lane (hw2): SEGV-history ring — (tid, fault jiffies) for
// the last 64 delivered user SIGSEGVs. The holder-exit print in do_exit
// reads it: a thread that took a HANDLED SIGSEGV (FFRT's arena/stack
// handler consumed it) and then exits "cleanly" is the wedge's first
// face (hw4 tid 477: boundary-SEGV at 9.0 s, thread gone by 34 s with
// the process alive), and without this ring the exit looks voluntary.
static SEGV_HIST_TID: [core::sync::atomic::AtomicU32; 64] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 64];
static SEGV_HIST_J: [core::sync::atomic::AtomicU64; 64] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 64];
static SEGV_HIST_IX: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

fn segv_hist_note(fault_addr: u64) {
    let tid = crate::sched::hotpath::get_current_pid() as u32;
    if tid == 0 {
        return;
    }
    let i = SEGV_HIST_IX.fetch_add(1, core::sync::atomic::Ordering::Relaxed) % 64;
    SEGV_HIST_TID[i].store(tid, core::sync::atomic::Ordering::Relaxed);
    // Upper 32 bits carry cr2's low 32 (page id for correlation); lower
    // 32 carry jiffies (age at exit-print time).
    SEGV_HIST_J[i].store(
        ((fault_addr & 0xffff_ffff) << 32) | (crate::timer::get_jiffies() & 0xffff_ffff),
        core::sync::atomic::Ordering::Relaxed,
    );
}

/// Age (in jiffies) of this tid's most recent delivered SIGSEGV, plus
/// that fault's cr2, if the tid ever took one.
pub fn segv_hist_age(tid: u32) -> Option<(u64, u64)> {
    let now = crate::timer::get_jiffies();
    let mut best_j = 0u64;
    let mut best_addr = 0u64;
    let mut found = false;
    for i in 0..64 {
        if SEGV_HIST_TID[i].load(core::sync::atomic::Ordering::Relaxed) == tid {
            let w = SEGV_HIST_J[i].load(core::sync::atomic::Ordering::Relaxed);
            let j = w & 0xffff_ffff;
            if !found || j >= best_j {
                found = true;
                best_j = j;
                best_addr = w >> 32;
            }
        }
    }
    if found {
        Some((now.saturating_sub(best_j), best_addr))
    } else {
        None
    }
}

/// Fatal user-mode SIGSEGV forensics: the GP register set, the VMA
/// covering the faulting rip, and the first stack qwords (return
/// addresses) — enough to identify which mapping jumped through a
/// garbage pointer (the gnome-shell ELF-magic wild-jump class:
/// fault == 0x010102464c457f == LE bytes "\x7fELF\x02\x01\x01\x00").
/// Diagnostic only: every read is checked through the page-table walk.
static USEGV_FOR_ENTERED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);
// HOTPATH-PRINT (whole fn): fatal-path forensics — one dump per user
// SIGSEGV death; the sending task exits immediately after.
pub fn user_segv_forensics(regs: &PtRegs, fault_addr: u64) {
    // R13 (fr13's silent death): a re-entrancy latch for the forensic
    // path itself. The full-decode body probes userspace memory and
    // walks stacks; on a kernel whose memory is under an active
    // corruption hunt those reads CAN fault, and the fault re-entered
    // the trap path mid-forensics — fr13's serial ended MID-TRACE with
    // no park marker (the second fault's own dump also died), parking
    // CPUs silently and wedgeing the box with zero evidence. The KERNPANIC
    // dump has had exactly this latch since R73; this path did not.
    // Second entry: one raw marker, release any stranded runqueue, IRQs
    // off, eternal halt (the round-11 parked-CPU discipline — a dead CPU
    // never takes interrupts again).
    if USEGV_FOR_ENTERED.swap(true, core::sync::atomic::Ordering::Relaxed) {
        for &b in b"trap: user_segv_forensics re-entered; halting cpu=" {
            crate::console::uart_putc_raw(b);
        }
        {
            let c = crate::arch::cpu_id();
            if c < 10 {
                crate::console::uart_putc_raw(b'0' + c as u8);
            } else {
                crate::console::uart_putc_raw(b'A' + (c - 10) as u8);
            }
        }
        crate::console::uart_putc_raw(b'\n');
        crate::sched::hotpath::terminal_release_grq_if_held();
        crate::arch::cpu::disable_irq();
        // hlt with IF=0 never wakes — the intentional permanent park of
        // a crashed forensic context (the audited diag edge).
        crate::arch::x86_64::r#unsafe::diag_halt_forever();
    }
    segv_hist_note(fault_addr);
    let pid = crate::sched::hotpath::get_current_pid();
    // sc112: name the crashing process — the memfd USB DDK PnP NULL-walk
    // kills an unnamed samgr SA host; comm decides which image cfg to cut.
    let comm = crate::sched::hotpath::current()
        .map(|t| t.comm())
        .map(|c| core::str::from_utf8(c).unwrap_or(""))
        .unwrap_or("")
        .trim_end_matches('\0');
    // sc160: the crash-retry loop (AMS respawns the launcher, every child
    // dies at the same stale VA) turned this ~21-line report into a 14
    // k-line serial storm — and each printk holds the UART irqsave lock,
    // so the storm itself manufactured the 453-1000 ms IRQ-OFF windows
    // that kill foundation via its XCollie watchdog (681 reports in
    // sc159). Full forensics for the first 6 faults; after that a
    // one-liner that keeps the census and adds the one attribution that
    // matters: whether cr2 falls inside a live binder buffer mapping
    // (the range forked children lost with VmaFlags DONTFORK in sc157).
    let binder_hit = crate::ipc::binder::binder_mmap_owner_of(fault_addr);
    {
        static SEGVD_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        // huks-wedge 114th: the NULL-deref storm class (usb_host's
        // handled UBSAN family, cr2=0/low) burns the 6-decode budget in
        // the first seconds and the POOL-FAMILY faults (cr2 >= 0x7f00...
        // — the FFRT slab/guard geometry, the Edge-A root-cause sample)
        // then land as one-liners with no adjvma (hw8: three such faults
        // lost). Split the budget: low faults keep the 6-decode cap;
        // high-userspace faults get their own 8-decode budget.
        static SEGVD_HI_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        // early30 (120th): control-flow faults (instruction fetch at a
        // non-exec target — cr2 == rip, the foundation exec-at-heap and
        // the IMS call-to-0 faces) are the rare corruption-shaped ones,
        // but they are also low-address faults and the NULL-deref storm
        // (bluetooth/usb_host, cr2=0, rip!=cr2) burns the 6-decode low
        // budget long before they land (e30b6: foundation's one fault
        // got a one-liner). Own 8-decode bucket for rip==cr2.
        static SEGVD_XN_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        let hi_fault = fault_addr >= 0x7f00_0000_0000;
        // early30 follow-up: split the NULL class out of the low bucket
        // too — the bluetooth/usb_host storm faults at cr2=0 (rip!=cr2)
        // and its 8+ faults/boot starve every OTHER low fault (e30f5's
        // IMS heap-exec variant faulted at cr2=0x2ba96d50 with rip in
        // the heap — not the XN bucket, and the low budget was already
        // burned). e30g3 refined the split: the storm's cr2 is ALWAYS
        // exactly 0x0, while the heap-exec face dereferences a NEAR-NULL
        // address (cr2=0x38, a NULL->field access at offset 0x38) — so
        // the NULL bucket must match cr2 == 0 exactly, and every other
        // low fault (near-NULL included) keeps the 6-decode budget.
        static SEGVD_NULL_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        let over = if fault_addr == regs.rip {
            SEGVD_XN_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) >= 8
        } else if hi_fault {
            SEGVD_HI_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) >= 8
        } else if fault_addr == 0 {
            SEGVD_NULL_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) >= 2
        } else {
            SEGVD_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) >= 6
        };
        if over {
            // HOTPATH-PRINT: fatal-path forensics — once per user SIGSEGV death
            crate::pr_err!(
                "USEGV-L pid={} comm='{}' rip={:#x} cr2={:#x} {}",
                pid,
                comm,
                regs.rip,
                fault_addr,
                match binder_hit.as_ref() {
                    Some((proc_id, owner_pid, base, size)) => alloc::format!(
                        "BINDERBUF proc={} owner_pid={} base={:#x} size={:#x}",
                        proc_id,
                        owner_pid,
                        base,
                        size
                    ),
                    None => alloc::string::String::new(),
                }
            );
            return;
        }
    }
    if let Some((proc_id, owner_pid, base, size)) = binder_hit.as_ref() {
        // HOTPATH-PRINT: fatal-path forensics — once per user SIGSEGV death
        crate::pr_err!(
            "USEGV-FOR: cr2 {:#x} INSIDE BINDER BUFFER of proc {} (owner pid {}) base {:#x} size {:#x} — stale daemon mapping dropped at fork (DONTFORK)",
            fault_addr, proc_id, owner_pid, base, size
        );
    }
    // HOTPATH-PRINT: fatal-path forensics — once per user SIGSEGV death
    crate::pr_err!(
        "USEGV-FOR: pid={} comm='{}' rip={:#x} cr2={:#x} rsp={:#x} rbp={:#x}",
        pid, comm, regs.rip, fault_addr, regs.rsp, regs.rbp
    );
    // HOTPATH-PRINT: fatal-path forensics — once per user SIGSEGV death
    crate::pr_err!(
        "USEGV-FOR: rax={:#x} rbx={:#x} rcx={:#x} rdx={:#x} rsi={:#x} rdi={:#x}",
        regs.rax, regs.rbx, regs.rcx, regs.rdx, regs.rsi, regs.rdi
    );
    // HOTPATH-PRINT: fatal-path forensics — once per user SIGSEGV death
    crate::pr_err!(
        "USEGV-FOR: r8={:#x} r9={:#x} r10={:#x} r11={:#x} r12={:#x} r13={:#x} r14={:#x} r15={:#x}",
        regs.r8, regs.r9, regs.r10, regs.r11, regs.r12, regs.r13, regs.r14, regs.r15
    );

    // 49th (r41 crash-storm hunt): the faulting task's failing-syscall
    // tail (sc125 ring, nr packed low-16 / errno high-16, newest first)
    // plus the latched in-flight syscall. The usb_host NULL-deref fires
    // in an error-cleanup list walk right after a call returned -EBUSY
    // (rax pre-fault = -16); this names WHICH syscall returned it. Print
    // path only, same decode format as the futex census park print.
    if let Some(tk) = crate::sched::hotpath::current() {
        use core::fmt::Write;
        use core::sync::atomic::Ordering::Relaxed;
        let mut s = alloc::string::String::from("USEGV-FOR: fails:");
        let cur = tk.ext.last_fails_cur.load(Relaxed);
        for k in 0..8 {
            let i = (cur + 8 - 1 - k) % 8;
            let v = tk.ext.last_fails[i].load(Relaxed);
            if v != 0 {
                let a0 = tk.ext.last_fails_a0[i].load(Relaxed);
                let a1 = tk.ext.last_fails_a1[i].load(Relaxed);
                let _ = write!(
                    s,
                    " nr{}=-{} a0={:#x} a1={:#x}",
                    v & 0xffff,
                    v >> 16,
                    a0,
                    a1
                );
            }
        }
        if s.len() == "USEGV-FOR: fails:".len() {
            s.push_str(" (none recorded)");
        }
        let _ = write!(
            s,
            " | insys nr={} a0={:#x} a1={:#x}",
            tk.ext.last_syscall_nr.load(Relaxed),
            tk.ext.last_syscall_a0.load(Relaxed),
            tk.ext.last_syscall_a1.load(Relaxed)
        );
        // HOTPATH-PRINT: fatal-path forensics — once per user SIGSEGV death
        crate::pr_err!("{}", s);
    }

    // 50th (crash-storm hunt): the faulting task's address-space edit
    // ring plus the rings of every sibling thread in the faulting tgid.
    // The mallocng family writes a freshly-stamped group through a
    // pointer that lands in a VMA hole; whether some thread's mmap
    // RETURNED that address, some munmap REMOVED its covering VMA, or
    // nobody ever touched it (userspace stale chain) names the culprit
    // in one boot. Encoding in Task::mm_ops (dispatch.rs writer).
    {
        // HOTPATH-PRINT (whole fn): fatal-path forensics — one bounded dump
// per user SIGSEGV death (capped event counts inline).
fn dump_mm_ops(tag: &str, t: &crate::process::task::Task) {
            use core::sync::atomic::Ordering::Relaxed;
            const N: usize = 16;
            let cur = t.ext.mm_ops_cur.load(Relaxed);
            let mut shown = 0usize;
            for k in 0..N {
                let i = (cur + N - 1 - k) % N;
                let w0 = t.ext.mm_ops[i * 2].load(Relaxed);
                let w1 = t.ext.mm_ops[i * 2 + 1].load(Relaxed);
                let op = w0 & 0xff;
                if op == 0 {
                    continue; // slot never written
                }
                let rc = (w0 >> 8) & 0xff;
                let addr = w0 >> 16;
                let len = w1 & 0x0000_ffff_ffff_ffff;
                let prot = (w1 >> 48) & 0xff;
                let name = match op {
                    1 => "mmap",
                    2 => "mprotect",
                    3 => "munmap",
                    4 => "brk",
                    6 => "madvise",
                    _ => "mremap",
                };
                let pid = t.pid();
                if shown < 12 {
                    // HOTPATH-PRINT: fatal-path forensics — bounded dump for the SIGSEGV death report
                    crate::pr_err!(
                        "USEGV-FOR: mm[{tag} pid={}] {} addr={:#x} len={:#x} prot/arg2={:#x} rc={}",
                        pid, name, addr, len, prot, rc
                    );
                }
                shown += 1;
            }
            if shown > 12 {
                // HOTPATH-PRINT: fatal-path forensics — bounded dump for the SIGSEGV death report
                crate::pr_err!("USEGV-FOR: mm[{tag}] ... {}/16 events (capped)", shown);
            }
        }
        if let Some(tk) = crate::sched::hotpath::current() {
            dump_mm_ops("self", tk);
            let tgid = tk.tgid();
            let self_ptr = tk as *const crate::process::task::Task;
            let mut sibs = 0usize;
            let skipped = crate::sched::hotpath::for_each_task_try(|t| {
                if t.tgid() == tgid && (t as *const crate::process::task::Task) != self_ptr {
                    sibs += 1;
                    if sibs <= 6 {
                        dump_mm_ops("sib", t);
                    }
                }
            });
            if sibs > 6 {
                // HOTPATH-PRINT: fatal-path forensics — bounded dump for the SIGSEGV death report
                crate::pr_err!("USEGV-FOR: mm[sib] ... {}/{} siblings shown", 6, sibs);
            }
            if skipped > 0 {
                // HOTPATH-PRINT: fatal-path forensics — bounded dump for the SIGSEGV death report
                crate::pr_err!("USEGV-FOR: mm[sib] {} pid-hash buckets skipped (try-walk)", skipped);
            }
        }
    }

    // VMA covering the faulting rip (which library / JIT region ran).
    let addr_space = crate::sched::hotpath::current().and_then(|t| t.address_space());
    if let Some(aspace) = addr_space.as_ref() {
        let vmas = aspace.vma_read();
        // leg-27: also name the INDIRECT-CALL TARGET (rax at a CFI
        // slowpath fault = the callee whose shadow slice was touched)
        // and the stack VMA (rsp inside a file-backed read-only segment
        // would name a clobbered stack). Print-path only.
        for &probe in &[regs.rip, fault_addr, regs.rax, regs.rsp] {
            match vmas.find(crate::mm::page::VirtAddr::new(probe as usize)) {
                // HOTPATH-PRINT: fatal-path forensics — bounded dump for the SIGSEGV death report
                Some(v) => crate::pr_err!(
                    "USEGV-FOR: vma({:#x}) = {:#x}-{:#x} flags={:#x} type={:?} fd={} off={:#x} origin={}{}",
                    probe,
                    v.start().as_usize(),
                    v.end().as_usize(),
                    v.flags().bits(),
                    v.vma_type(),
                    v.file_fd(),
                    v.offset(),
                    v.origin().creator_name(),
                    if v.origin().is_forked() { "+FORK" } else { "" }
                ),
                // HOTPATH-PRINT: fatal-path forensics — bounded dump for the SIGSEGV death report
                None => crate::pr_err!("USEGV-FOR: vma({:#x}) = NONE", probe),
            }
        }
        // File path + module-relative fault offset for the rip VMA, so a
        // crash inside a mapped .so names the library and gives the
        // addr2line offset directly (sc49: appspawn children die inside a
        // FileBacked VMA and fd/offset alone leaves the crash site
        // anonymous).
        if let Some(v) = vmas.find(crate::mm::page::VirtAddr::new(regs.rip as usize)) {
            let fd = v.file_fd();
            if fd >= 0 {
                // SAFETY: current task's fd table is alive for the
                // duration of the fault report.
                let path = crate::fs::file::get_file_fd(fd as usize)
                    .map(|f| f.path())
                    .unwrap_or_else(|| alloc::string::String::from("?"));
                // HOTPATH-PRINT: fatal-path forensics — bounded dump for the SIGSEGV death report
                crate::pr_err!(
                    "USEGV-FOR: ripfile {} voff {:#x}",
                    path,
                    v.offset() + (regs.rip as usize - v.start().as_usize())
                );
                // sc106 OsAccount hunt: the recurring SA-host crash
                // (identical rip, NULL list walk) runs inside a memfd-
                // hosted library the offline tooling cannot name. Its
                // .dynstr/.rodata usually sits in the read-only segment
                // mapped directly below the r-x segment — scan up to
                // 64 KiB below the mapping for printable runs so the
                // crash site names its library (unmapped gaps stop the
                // scan safely; page-table walks never fault).
                // cfi4 (22nd): the scan must also fire when the path is
                // unrecoverable for ANY reason, not just the anon/memfd
                // class — lb348's usb_host crasher mapped a file-backed
                // r-x VMA whose fd was already closed (ripfile "?", voff
                // 0x15bac, bytes absent from every image = memfd/removed
                // file). Same scan, same caps.
                if path.contains("anon") || path == "?" {
                    let mut line = alloc::string::String::from("USEGV-FOR: memfd-strs");
                    use core::fmt::Write;
                    let mut hits = 0usize;
                    let mut run = alloc::vec::Vec::new();
                    let start_va = (v.start().as_usize() as u64).saturating_sub(0x1_0000);
                    let end_va = v.start().as_usize() as u64;
                    let mut va = start_va;
                    'scan: while va < end_va {
                        match fore_read_u64(aspace.root_ppn(), va) {
                            Some(w) => {
                                for k in 0..8usize {
                                    let b = (w >> (k * 8)) & 0xff;
                                    if (0x20..0x7f).contains(&b) {
                                        run.push(b as u8);
                                    } else {
                                        if run.len() >= 10 && hits < 8 {
                                            let _ = write!(
                                                line,
                                                " [{}]",
                                                core::str::from_utf8(&run)
                                                    .unwrap_or("?")
                                            );
                                            hits += 1;
                                        }
                                        run.clear();
                                        if hits >= 8 {
                                            break 'scan;
                                        }
                                    }
                                }
                            }
                            None => {
                                // Unmapped gap — skip 4 KiB ahead.
                                va = (va & !0xfff) + 0x1000;
                                continue;
                            }
                        }
                        va += 8;
                    }
                    // HOTPATH-PRINT: fatal-path forensics — bounded dump for the SIGSEGV death report
                    crate::pr_err!("{}", line);
                }
            }
        }
        // Instruction bytes at rip: the dynamic linker CLOSES the mmap fd
        // after mapping, so the path is often unrecoverable from the VMA —
        // the bytes identify the exact function offline (match against the
        // .so files extracted from the system/vendor images).
        {
            let mut bytes = [0u8; 16];
            let mut ok = true;
            for (i, b) in bytes.iter_mut().enumerate() {
                match fore_read_u64(aspace.root_ppn(), regs.rip + i as u64) {
                    Some(w) => *b = (w & 0xff) as u8,
                    None => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                // HOTPATH-PRINT: fatal-path forensics — bounded dump for the SIGSEGV death report
                crate::pr_err!(
                    "USEGV-FOR: ripbytes {:02x?}",
                    bytes
                );
            }
        }
        // VMA neighborhood around the fault address (render_service
        // offset-anomaly family, session 18): the single vma(cr2) line
        // cannot show what borders the faulting region — whether a
        // writable stack sits directly below a mis-flagged PROT_NONE
        // tail, a guard page, or a hole. Dump every VMA overlapping a
        // +/-16MB window around cr2, capped at 12 entries (fatal path,
        // rare; bounded by construction).
        {
            let lo = (fault_addr as usize).saturating_sub(0x100_0000);
            let hi = (fault_addr as usize).saturating_add(0x100_0000);
            let mut shown = 0usize;
            for v in vmas.iter() {
                if v.end().as_usize() <= lo || v.start().as_usize() >= hi {
                    continue;
                }
                if shown >= 12 {
                    // HOTPATH-PRINT: fatal-path forensics — bounded dump for the SIGSEGV death report
                    crate::pr_err!("USEGV-FOR: nearvma ... (capped)");
                    break;
                }
                // HOTPATH-PRINT: fatal-path forensics — bounded dump for the SIGSEGV death report
                crate::pr_err!(
                    "USEGV-FOR: nearvma {:#x}-{:#x} flags={:#x} type={:?} fd={} off={:#x} origin={}{}",
                    v.start().as_usize(),
                    v.end().as_usize(),
                    v.flags().bits(),
                    v.vma_type(),
                    v.file_fd(),
                    v.offset(),
                    v.origin().creator_name(),
                    if v.origin().is_forked() { "+FORK" } else { "" }
                );
                shown += 1;
            }
        }
        // Edge A root-cause (114th, stack-layout lead): the ±16MB
        // window above caps out long before cr2 in the densely-mapped
        // huks region (the pool base sits ~0x31000 above the linker
        // mapping chain). A tight ±256KB "adjvma" pass prints EVERY
        // slice/guard around the fault with sizes spelled out — the
        // pooled-stack geometry (repeating RW-slice/guard units, their
        // origins) that decides whether the straddling copy could ever
        // have been readable under a different placement. Capped at 24
        // entries; runs in the same budgeted full-forensics branch.
        {
            let lo = (fault_addr as usize).saturating_sub(0x4_0000);
            let hi = (fault_addr as usize).saturating_add(0x4_0000);
            let mut shown = 0usize;
            for v in vmas.iter() {
                if v.end().as_usize() <= lo || v.start().as_usize() >= hi {
                    continue;
                }
                if shown >= 24 {
                    // HOTPATH-PRINT: fatal-path forensics — bounded dump for the SIGSEGV death report
                    crate::pr_err!("USEGV-FOR: adjvma ... (capped)");
                    break;
                }
                // HOTPATH-PRINT: fatal-path forensics — bounded dump for the SIGSEGV death report
                crate::pr_err!(
                    "USEGV-FOR: adjvma {:#x}-{:#x} sz={:#x} flags={:#x} type={:?} fd={} off={:#x} origin={}{}",
                    v.start().as_usize(),
                    v.end().as_usize(),
                    v.end().as_usize() - v.start().as_usize(),
                    v.flags().bits(),
                    v.vma_type(),
                    v.file_fd(),
                    v.offset(),
                    v.origin().creator_name(),
                    if v.origin().is_forked() { "+FORK" } else { "" }
                );
                shown += 1;
            }
            // Altstack membership: OH musl reserves a per-thread signal
            // stack; if rsp sits inside it, the fault happened in a
            // signal handler and the KERNEL's frame placement (top of
            // ss region minus SIGNAL_FRAME_SIZE) is in the causal chain.
            if let Some(tk) = crate::sched::hotpath::current() {
                // Read-only fields of the current task (typed handle).
                let ss = &tk.sigstack;
                let ss_on = !ss.is_disabled() && ss.ss_sp != 0;
                let (ss_sp, ss_size) = (ss.ss_sp, ss.ss_size);
                let rsp_in = ss_on
                    && (regs.rsp as u64) >= ss_sp
                    && (regs.rsp as u64) < ss_sp.saturating_add(ss_size);
                let cr2_in = ss_on && fault_addr >= ss_sp && fault_addr < ss_sp.saturating_add(ss_size);
                // HOTPATH-PRINT: fatal-path forensics — bounded dump for the SIGSEGV death report
                crate::pr_err!(
                    "USEGV-FOR: altstack on={} sp={:#x} sz={:#x} rsp_in={} cr2_in={}",
                    ss_on as u8,
                    ss_sp,
                    ss_size,
                    rsp_in as u8,
                    cr2_in as u8
                );
            }
        }
    }

    // Stack words (return-address chain): read through the page-table
    // walk so a bad/absent rsp cannot take the kernel down.
    // HOTPATH-PRINT (whole fn): fatal-path forensics — bounded stack reads
// for the SIGSEGV death report.
fn fore_read_u64(root_ppn: u64, va: u64) -> Option<u64> {
        // Read-only page-table walk + volatile read on a live root (the
        // audited diag edge carries both).
        crate::arch::x86_64::r#unsafe::diag_fore_read_u64(root_ppn, va)
    }
    if let Some(aspace) = addr_space.as_ref() {
        let root = aspace.root_ppn();
        let base = (regs.rsp & !0xf) as u64;
        let mut line = alloc::string::String::new();
        for i in 0..8usize {
            let va = base + (i * 8) as u64;
            let val = fore_read_u64(root, va)
                .map(|v| alloc::format!("{:#x}", v))
                .unwrap_or(alloc::string::String::from("????????????????"));
            use core::fmt::Write;
            let _ = write!(line, " {}:{}", i, val);
        }
        // HOTPATH-PRINT: fatal-path forensics — bounded stack read for the death report
        crate::pr_err!("USEGV-FOR: stack@rsp:{}", line);
        // leg-31B: name the VMA (+origin) behind each stack word that
        // looks like a user code pointer — the faulting frame's return
        // addresses name the CALLER library without a guest-side
        // backtracer (the render_service guard-read fault died inside
        // ld-musl memcpy; the caller's return address was the only
        // attribution clue and it lived on the stack).
        if let Some(vmas2) = addr_space.as_ref().map(|a| a.vma_read()) {
            for i in 0..8usize {
                let va = match base.checked_add((i * 8) as u64) {
                    Some(v) => v,
                    None => break,
                };
                if let Some(val) = fore_read_u64(root, va) {
                    if val <= 0x1000 || val >= 0x0000_8000_0000_0000 {
                        continue;
                    }
                    if let Some(v) = vmas2
                        .find(crate::mm::page::VirtAddr::new(val as usize))
                    {
                        // Only code-looking mappings (r-x / rwx).
                        let f = v.flags();
                        if f.is_executable() {
                            // Name the backing: fd or the pinned vm_file
                            // (memfd-loaded libraries are FileBacked with
                            // an anon_inode path — the pin is the only
                            // name they have).
                            let mut backing =
                                alloc::string::String::new();
                            if let Some(fl) = aspace.get_vma_file(
                                v.start().as_usize(),
                            ) {
                                use core::fmt::Write;
                                let _ = write!(
                                    backing,
                                    " path={}",
                                    fl.path()
                                );
                            } else if v.file_fd() >= 0 {
                                use core::fmt::Write;
                                let _ = write!(
                                    backing,
                                    " fd={}",
                                    v.file_fd()
                                );
                            }
                            // HOTPATH-PRINT: fatal-path forensics — bounded stack read for the death report
                            crate::pr_err!(
                                "USEGV-FOR: stkva[{}] {:#x} = {:#x}-{:#x} r-x off={:#x}{} origin={}{}",
                                i,
                                val,
                                v.start().as_usize(),
                                v.end().as_usize(),
                                v.offset(),
                                backing,
                                v.origin().creator_name(),
                                if v.origin().is_forked() { "+FORK" } else { "" }
                            );
                            // sc324 follow-up: code bytes at the return
                            // site (16 before, 8 after) read through the
                            // page-table walk — fingerprints the caller
                            // even for memfd-backed libraries with no
                            // on-disk copy to compare against.
                            let mut cb = alloc::string::String::new();
                            use core::fmt::Write;
                            let _ = write!(cb, "USEGV-FOR: stkcode[{}]", i);
                            let lo = (val - 16) & !7;
                            for k in 0..3u64 {
                                match fore_read_u64(root, lo + k * 8) {
                                    Some(w) => {
                                        let _ = write!(cb, " {:#x}", w);
                                    }
                                    None => {
                                        let _ = write!(cb, " :?");
                                    }
                                }
                            }
                            // HOTPATH-PRINT: fatal-path forensics — bounded stack read for the death report
                            crate::pr_err!("{}", cb);
                        }
                    }
                }
            }
        }
    }

    // Latch discipline (L7 mmap-family triage, 2026-11-21): release the
    // re-entrancy latch on every CLEAN forensics exit. The swap(true)
    // arm above used to stay latched forever after one dump, so any
    // LATER fatal user SIGSEGV on the boot re-entered the halt path and
    // parked its CPU — turning every subsequent segv-path bug into a
    // whole-boot wedge with the forensics silenced (the mmap05->shmdt01
    // pair-reproducer face). The recursion guard itself is unchanged:
    // a fault WHILE this dump is in flight still takes the fr13 halt.
    USEGV_FOR_ENTERED.store(false, core::sync::atomic::Ordering::Release);
}


// ===========================================================================
// ripsample/stime report formatters (moved from trap.rs; the sampler
// statics and the tick-path writers stay framework-side, published for
// these readers)
// ===========================================================================

pub fn utime_ripsample_report() {
    use core::sync::atomic::Ordering::Relaxed;
    if !crate::trace::EVENT_RIPSAMPLE.armed() {
        return;
    }
    static LAST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let j = crate::drivers::timer::x86_64::get_jiffies();
    if j < LAST.load(Relaxed) + 60 * crate::config::KERNEL_HZ as u64 {
        return;
    }
    LAST.store(j, Relaxed);
    let u = crate::arch::x86_64::trap::UTIME_LAUNCHER_TICKS.load(Relaxed);
    let s = crate::arch::x86_64::trap::STIME_LAUNCHER_TICKS.load(Relaxed);
    // HOTPATH-PRINT: one-shot report (armed by ripsample boot param)
    crate::pr_warn!("RIPSAMPLE: launcher utime={} stime={}", u, s);
    for i in 0..crate::arch::x86_64::trap::UTIME_SLOTS {
        let p = crate::arch::x86_64::trap::UTIME_HIST[i].page.load(Relaxed);
        let c = crate::arch::x86_64::trap::UTIME_HIST[i].count.load(Relaxed);
        if p != 0 && c > 0 {
            // HOTPATH-PRINT: one-shot report (armed by ripsample boot param)
            crate::pr_warn!("RIPSAMPLE: u-page={:#x} n={}", p, c);
        }
    }
    for i in 0..crate::arch::x86_64::trap::STIME_SLOTS {
        let p = crate::arch::x86_64::trap::STIME_FOCUS[i].page.load(Relaxed);
        let c = crate::arch::x86_64::trap::STIME_FOCUS[i].count.load(Relaxed);
        if p != 0 && c > 0 {
            let mut comm = [0u8; 16];
            for jx in 0..16 {
                comm[jx] = crate::arch::x86_64::trap::STIME_FOCUS[i].comm[jx].load(Relaxed);
            }
            let name = core::str::from_utf8(&comm[..]).unwrap_or("").trim_end_matches('\0');
            // HOTPATH-PRINT: one-shot report (armed by ripsample boot param)
            crate::pr_warn!("RIPSAMPLE: k-page={:#x} n={} comm={}", p, c, name);
        }
    }
}

pub fn stime_profile_top(focus: bool) -> alloc::vec::Vec<([u8; 16], u64, u64)> {
    use core::sync::atomic::Ordering::Relaxed;
    let hist: &[crate::arch::x86_64::trap::StimeSample; crate::arch::x86_64::trap::STIME_SLOTS] = if focus {
        &crate::arch::x86_64::trap::STIME_FOCUS
    } else {
        &crate::arch::x86_64::trap::STIME_HIST
    };
    let mut v = alloc::vec::Vec::new();
    for i in 0..crate::arch::x86_64::trap::STIME_SLOTS {
        let page = hist[i].page.load(Relaxed);
        if page == 0 {
            continue;
        }
        let mut comm = [0u8; 16];
        for j in 0..16 {
            comm[j] = hist[i].comm[j].load(Relaxed);
        }
        v.push((comm, page, hist[i].count.load(Relaxed)));
    }
    v.sort_by(|a, b| b.2.cmp(&a.2));
    v
}

// ===========================================================================
// Cross-stack scribble foreign-entry report (moved from trap.rs)
// ===========================================================================

fn scribble_put_hex(mut v: u64) {
    for _ in 0..16 {
        let n = (v >> 60) as u8;
        crate::console::putchar_no_lock(if n < 10 { b'0' + n } else { b'a' + n - 10 });
        v <<= 4;
    }
}

fn scribble_put_str(s: &[u8]) {
    for &b in s {
        crate::console::putchar_no_lock(b);
    }
}

/// Raw (lock-free) report of a user-origin trap frame landing outside the
/// current task's kernel stack — the cross-stack scribble engine, named at
/// the moment of entry with the full per-CPU ownership picture.
pub fn scribble_foreign_entry(
    cpu: usize,
    vector: u64,
    regs: *mut PtRegs,
    tp: *mut crate::process::task::Task,
    frame: usize,
) {
    scribble_put_str(b"\n=== FOREIGN-ENTRY cpu=");
    scribble_put_hex(cpu as u64);
    scribble_put_str(b" vector=0x");
    scribble_put_hex(vector);
    scribble_put_str(b" frame=0x");
    scribble_put_hex(frame as u64);
    // Diagnostic header reads of the task named by the slot (the
    // audited diag edge adopts the borrow — broken-machine report,
    // short ops, the CPU parks after).
    let (task_ptr, task_pid, kbottom, ktop) =
        crate::arch::x86_64::r#unsafe::diag_task_fields(tp);
    {
        scribble_put_str(b" task=0x");
        scribble_put_hex(task_ptr);
        scribble_put_str(b" pid=");
        scribble_put_hex(task_pid as u64);
        scribble_put_str(b" kstack=[0x");
        scribble_put_hex(kbottom as u64);
        scribble_put_str(b",0x");
        scribble_put_hex(ktop as u64);
        scribble_put_str(b") rsp0_mirror=0x");
        scribble_put_hex(
            crate::arch::x86_64::smp::PER_CPU[cpu]
                .tss_rsp0
                .load(core::sync::atomic::Ordering::Relaxed),
        );
        scribble_put_str(b" TSS.rsp0=0x");
        scribble_put_hex(crate::arch::x86_64::trap::diag_tss_rsp0(cpu));
        scribble_put_str(b"\nFRAME:");
        for i in 0..21usize {
            if i % 4 == 0 {
                scribble_put_str(b"\n  ");
            }
            scribble_put_str(b" ");
            // Frame walk of the foreign stack — best-effort forensic
            // read through the audited edge (the CPU parks after this
            // report).
            scribble_put_hex(
                crate::arch::x86_64::r#unsafe::diag_volatile_read_u64(frame + i * 8),
            );
        }
        // Frame surroundings: the slots just above the frame belong to the
        // stack owner's earlier execution — torn words there name what the
        // two interleaved contexts were doing.
        scribble_put_str(b"\nABOVE:");
        let mut a = frame + 0xa8;
        while a < frame + 0xa8 + 0x100 {
            scribble_put_str(b" ");
            scribble_put_hex(crate::arch::x86_64::r#unsafe::diag_volatile_read_u64(a));
            a += 8;
        }
    }
    scribble_put_str(b"\nSLOTS:");
    for c in 0..crate::config::MAX_CPUS {
        let t = crate::arch::x86_64::smp::PER_CPU[c]
            .current_task
            .load(core::sync::atomic::Ordering::Relaxed);
        scribble_put_str(b" c");
        let mut cc = c as u64;
        if cc >= 10 {
            crate::console::putchar_no_lock(b'0' + (cc / 10) as u8);
            cc %= 10;
        }
        crate::console::putchar_no_lock(b'0' + cc as u8);
        scribble_put_str(b"=0x");
        scribble_put_hex(t);
    }
    scribble_put_str(b"\n");
    crate::debug::scribble::ring_dump();
}
