//! The BINDER COLD DIAGNOSTIC HALVES (moved from
//! kernel/core/src/ipc/binder.rs at the cold-four policy flip — sole copies
//! here; zero behavior change): the ~1 Hz heartbeat audits
//! (txn_hang_audit, bwr_park_audit, ams46_audit, pool_census_audit),
//! the sleep/stall census family (sleep_snapshot, stall_report,
//! schb_beat, schb_waiter_census + their formatting helpers), the
//! latched-map vma dump + park-chain latch walks, the holder/stall
//! thread-line formatters, the BC command decoder and the app-child
//! registry write. Their framework call sites are the heartbeat slot
//! (through binder_diag_hooks rows), taskdump/hung_task/schb (rows),
//! and rux-obs / syscall (direct — the allowed downward edge).
//!
//! What the moved code consumes from the framework (the published
//! surface, all on kernel/core/src/ipc/binder.rs + unsafe.rs):
//! - the world machinery: world_lock / world_lock_try / World /
//!   WorldGuard (the audits snapshot under WORLD exactly as before);
//! - the object model (BinderProc / BinderThread / BinderTxn / Work /
//!   BinderAlloc) — READ-only walks plus the documented OWING/
//!   AMS46/TXN counter publishes;
//! - the audited walker edges (unsafe.rs): entry_frame*,
//!   probe_*, stack_word, diag_task_comm, diag_task — the trap-regs /
//!   page-table / raw-task unsafe stayed framework-side.
//!
//! The txn mechanism (ioctl/thread_write/transaction/thread_read,
//! buffer alloc, node/ref lifecycle, FileOps, mmap) stays
//! framework-side — the recorded shelving verdict stands; this file
//! is the cold half AROUND it.

// The published binder surface this file consumes (see the header):
// the object model + world machinery from the framework's binder
// module, the audited walker edges, and the framework roots.
#[allow(unused_imports)]
use crate::binder::{
    BinderProc, BinderThread, BinderTxn, Work, World, WorldGuard, TxnBlock,
    BHIST, LOOPER_REGISTERED, LOOPER_ENTERED, LOOPER_EXITED,
    TF_ONE_WAY, TF_ACCEPT_FDS, BINDER_WRITE_READ,
    EV_BWR, EV_REKICK,
    TXN_OWING_SPID, TXN_OWING_STID, TXN_OWING_TXN, TXN_OWING_IX,
    AMS46_NODE_ID, AMS46_TPID, AMS46_T0, AMS46_SENDER, AMS46_READER,
    AMS46_RIP_N, AMS46_TXNS, AMS46_ENQ_N,
    APP_CHILD_PIDS,
    ams46_on, ams46_node_id, ams46_txns_contains, ams46_txns_push,
    is_inst, is_acct, txnhunt_on, pool_census_on,
    binder_sareg_samgr_pid, pm, world_lock, world_lock_try,
    hist_snapshot, hist_line_of, sweep_silent_txns,
    wake_thread, wake_proc_one,
};
#[allow(unused_imports)]
use crate::r#unsafe::{
    entry_frame, entry_frame_sp, probe_leaf,
    probe_u32, probe_u64, stack_word, diag_task_comm, diag_task,
};
#[cfg(feature = "x86_64")]
use crate::r#unsafe::entry_frame_futex_x86;
#[allow(unused_imports)]
use alloc::sync::Arc;
#[allow(unused_imports)]
use alloc::vec::Vec;


/// One-line summary of a thread for the stall dump (b85 follow-up: the
/// wedge state was invisible — "1 txn(s) stacked" said nothing about
/// what the txn was or why nobody took the proc todo).
/// SAFETY: caller holds WORLD.
pub fn stall_thread_line(
    world: &World,
    proc: &Arc<BinderProc>,
    t: &Arc<BinderThread>,
    out: &mut alloc::string::String,
) {
    use core::fmt::Write;
    let th = t.st(world);
    let looper = |l: u32| {
        let mut s = [b'-'; 3];
        if l & LOOPER_REGISTERED != 0 {
            s[0] = b'R';
        }
        if l & LOOPER_ENTERED != 0 {
            s[1] = b'E';
        }
        if l & LOOPER_EXITED != 0 {
            s[2] = b'X';
        }
        s
    };
    let lstr = looper(th.looper);
    let loop_str = core::str::from_utf8(&lstr).unwrap_or("-=-");
    // sc89: formatting (this fn) stays under WORLD; the slow parts —
    // console output and the holder-task page-table walks — run after
    // the guard drops (see holder_forensics + the flush at the end of
    // stall_report). sc82: the whole dump under the IRQ-saving guard
    // ran ~500 ms with irqs off and killed khungtaskd.
    match th.txn_stack.last() {
        Some(txn) => {
            let shape = if txn.to_tid(world) == Some(t.tid) {
                "SERVER-OWING"
            } else if txn.from_tid == Some(t.tid) {
                "CLIENT-WAIT"
            } else {
                "AMBIGUOUS"
            };
            let _ = writeln!(
                out,
                "  pid {} tid {} loop={} wait={} todo={} stack={} [{}] top=txn{} code {} oneway={} from pid {} tid {} to_tid {}",
                proc.pid,
                t.tid,
                loop_str,
                th.waiting_for_proc_work,
                th.todo.len(),
                th.txn_stack.len(),
                shape,
                txn.id,
                txn.code,
                txn.flags & TF_ONE_WAY != 0,
                txn.from_proc.as_ref().map(|p| p.pid).unwrap_or(0),
                txn.from_tid.unwrap_or(0),
                txn.to_tid(world).unwrap_or(0),
            );
        }
        None => {
            let _ = writeln!(
                out,
                "  pid {} tid {} loop={} wait={} todo={} stack=0",
                proc.pid,
                t.tid,
                loop_str,
                th.waiting_for_proc_work,
                th.todo.len(),
            );
        }
    }
}

/// Holder-task forensics (b128-b135: a samgr thread parks 120s+ INSIDE
/// a txn and every display-chain client queues behind it). The binder
/// view can't say where the thread actually is, so read its Task:
/// scheduling state plus the user entry frame at kernel_stack_top -
/// PT_REGS_SIZE (both arches place the user entry frame at that fixed
/// offset — see the pt_regs module docs). Reads ONLY scheduler/futex
/// state and the holder's own page tables — sc89: safe (and required)
/// to run OUTSIDE the WORLD guard; `t.tid` is immutable.
/// (unsafe-slim r3) un-signed: `t` is an Arc pin (no raw contract left).
// ============================================================================
// INV-BINDER-ENTRYFRAME (unsafe-slim r3): the ONE audited entry-frame read
// span in this file. Diagnostic decoders (sleep_snapshot / stall_report /
// txn_hang_audit / bwr_park_audit / ams46_audit / holder_forensics /
// user_task_park_stack_w) used to repeat a per-arch unsafe pt_regs deref
// each; the precondition is discharged once here instead (INV-SAFECALL-1).
// The frame slot is written by the arch entry path on every syscall entry;
// these helpers only READ it, and top == 0 (stack-freed guard, sc261 —
// the kernel stack is freed at thread exit while the Task struct survives
// until its last pin) returns zeros instead of touching the frame.
// ============================================================================

/// Diagnostic entry-frame read: (pc, syscall-no, a0, a1, a2).
/// Zeros when `top == 0` (see the INV-BINDER-ENTRYFRAME header).
#[inline]

pub fn holder_forensics(t: &Arc<BinderThread>, out: &mut alloc::string::String) {
    use core::fmt::Write;
    // Safe task lookup (INV: process::find_task_by_pid pins under the
    // pid-hash bucket; the &'static mut lives for this diagnostic).
    let Some(tk) = crate::process::find_task_by_pid(t.tid) else {
        return;
    };
    let comm = {
        let c = tk.comm();
        core::str::from_utf8(&c[..])
            .unwrap_or("")
            .trim_end_matches('\0')
    };
    // sc261: an exited thread's kernel stack is back in the cache and
    // kernel_stack_top() reports KERNEL_STACK_SIZE — an entry-frame
    // read then page-faults the kernel (sc261, the kill_dump path
    // died exactly so). The old SAFETY claim "the kernel stack stays
    // mapped while the Task lives" was wrong: the stack is freed at
    // thread exit while the Task struct survives until its last pin.
    // entry_frame() keeps the top == 0 zero-guard for this.
    let top = if tk.owns_kernel_stack() {
        tk.kernel_stack_top()
    } else {
        0
    };
    // (pc, sys, a0, a1, a2) — sub-tuple used below.
    let (rip, sys, a0, a1, _) = entry_frame(top);
    let _ = writeln!(
        out,
        "   holder task comm='{}' state={:#x} rip={:#x} sys={:#x} a0={:#x} a1={:#x}",
        comm,
        tk.state().bits(),
        rip,
        sys,
        a0,
        a1
    );
    // If the holder is parked in futex_wait, correlate its
    // waiter entry: woken=true-but-still-asleep indicts the
    // wake→run path; woken=false indicts the key match (key_mm
    // vs the task's own current identity) or the releaser.
    if let Some((uaddr, key_mm, woken)) =
        crate::sync::futex::waiter_of(tk as *mut crate::process::task::Task)
    {
        let _ = writeln!(
            out,
            "   futex waiter uaddr={:#x} key_mm={:#x} own_mm={:#x} woken={}",
            uaddr,
            key_mm,
            crate::sync::futex::task_futex_mm_id(tk as *mut crate::process::task::Task),
            woken
        );
        // sc75: read the futex WORD through the HOLDER's own
        // page tables (khungtaskd's get_user would use the
        // wrong mm). The word says whether the lock is still
        // owned (pthread mutexes store the owner tid) or
        // already released — a released word plus a sleeping
        // waiter is a lost futex wake; an owned word names the
        // owner to chase.
        let word = tk
            .address_space()
            .and_then(|aspace| probe_u32(aspace.root_ppn(), uaddr as u64));
        match word {
            Some(v) => {
                // Dump the neighbor words too (libc lock structs
                // pack owner/counts beside the futex word — musl
                // recursive/rwlock layouts need them to decode).
                let mut nb = [0u32; 4];
                let mut got = 0;
                for (k, slot) in nb.iter_mut().enumerate() {
                    let addr = uaddr + 4 + 4 * k;
                    if let Some(x) = tk
                        .address_space()
                        .and_then(|aspace| probe_u32(aspace.root_ppn(), addr as u64))
                    {
                        *slot = x;
                        got += 1;
                    }
                }
                let _ = writeln!(
                    out,
                    "   futex word = {:#x} (owner tid {}) next[{}]={:08x?}",
                    v, v, got, nb
                );
                // Chase a plausible owner tid (glibc stores the
                // owner TID in the mutex word for
                // error-checking/recursive mutexes).
                if v > 0 && v < 0x00ff_ffff {
                    if let Some(ot) = crate::process::find_task_by_pid(v) {
                        // Stack-freed guard (KILLDUMP census race class):
                        // the owner may have exited and freed its stack
                        // before this lookup — skip, never fault.
                        if let Some(otop) = ot.kernel_stack_top_checked() {
                            // (sys, pc) — entry_frame gives (pc, sys, ..).
                            let (pc, osys, _, _, _) = entry_frame(otop);
                            let orip = pc;
                            let _ = writeln!(
                                out,
                                "   owner tid {} state={:#x} sys={:#x} rip={:#x}",
                                v,
                                ot.state().bits(),
                                osys,
                                orip
                            );
                        } else {
                            let _ = writeln!(
                                out,
                                "   owner tid {} stack-freed (dump skipped)",
                                v
                            );
                        }
                    } else {
                        let _ = writeln!(
                            out,
                            "   owner tid {} NOT FOUND (exited while holding?)",
                            v
                        );
                    }
                }
            }
            None => {
                let _ = writeln!(out, "   futex word unreadable (page absent)");
            }
        }
    }
}

/// Binder stall scan, called from khungtaskd (process context) alongside
/// the D-state scan. A thread parked in the blocking binder read for
/// longer than the threshold is almost always an IPC chain stall —
/// typically a service that never answered a synchronous transaction.
/// S-state tasks are invisible to the D-state detector, so report these
/// here: pid/tid, wait duration, and — when the thread is waiting for a
/// REPLY — the pid of the process that owes it (txn_stack top's owner).
/// One report per wait episode. The first owing process gets a full
/// per-thread dump (proc todo depth, looper/wait/stack state of every
/// worker) so the next wedge names the exact thread that ate the wake.
/// rux:sleepsnap (retired debug=sleepsnap) (b11 appspawn wedge hunt): one line per S-state task —
/// comm/tid/state, the syscall number the thread is parked in, its rip
/// and first two args, read from the fixed user entry frame at
/// kernel_stack_top - PT_REGS_SIZE (same contract as the holder-task
/// forensics in stall_thread_line). The appspawn server swallows client
/// requests (write OK, reply never comes, no fork) while its kickdog
/// thread lives: this names the exact syscall the server main thread is
/// sleeping in, at khungtaskd cadence. Purely diagnostic; OFF unless
/// rux:sleepsnap (retired debug=sleepsnap) is on the cmdline.
/// Park-walk latch ring (54th hunt): tgids whose threads get the
/// ufw/uss user-frame walk in sleep_snapshot. Latched directly on
/// main-stack futex waits, and from the txn-hang audit for hung-txn
/// SERVER processes (their handler parks userspace-side before
/// replying — e.g. the device_manager InitDeviceManager reply the
/// distributeddata store's OnStart waits on).
static VMA_DUMPED_TGID: [core::sync::atomic::AtomicU32; 16] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 16];

/// 55th: STICKY latch ring — written ONLY by the sleep_snapshot
/// main-stack-untimed rule (the forever class: a thread parked on its
/// own main stack with NO timeout is the wedge signature). The churned
/// 16-slot ring above is shared with the 1 Hz txn-hang audit (~60
/// wedged tgids at the cascade's peak); a fresh latch could be evicted
/// between insert and the walk-gate scan (ohc45/49: samgr's main-stack
/// park observed at three consecutive snapshots, zero walks). Sticky
/// slots never evict — at most 8 distinct wedged-leader tgids.
static VMA_STICKY_TGID: [core::sync::atomic::AtomicU32; 8] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 8];

/// Dump-once ring shared by the softirq snapshot dumper and the
/// censusd task-context dumper (see dump_latched_maps_taskctx).
/// 97th (hp97 toxicity, round 2): hard per-boot budget on "uvma:" map
/// lines across BOTH emit sites (the softirq inline dump and
/// dump_latched_maps_taskctx). The DONE ring is 16 slots; a wedge that
/// latches more tgids than that thrashes the marks (evict-insert cycle)
/// and the taskctx dumper re-dumped the evicted maps every 250ms round —
/// hp97i/j printed 15.5k uvma lines (~2 MB) and the guest HARD-FROZE
/// mid-flood (serial silent from 281s, drivers blocked on read1). The
/// dump gates now latch only wedge-shaped parks, but any future gate
/// regression must hit this wall, not the machine.
static UVMA_LINE_BUDGET: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(600);
static VMA_DUMPED_DONE: [core::sync::atomic::AtomicU32; 16] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 16];

/// 56th: task-context map dump for latched tgids. The softirq
/// sleep_snapshot's vma_try_read NEVER succeeded for samgr in ohc53
/// (24 latched walks at t=254s, zero uvma lines, 250+ s of retries —
/// the non-blocking read loses whenever the target's vma lock is
/// held, and the wedged boot holds it often). censusd runs in kthread
/// context where the sleepable rwsem can be taken for read WITHOUT
/// spinning; this walks both latch rings, and for every latched tgid
/// lacking a done-mark emits its exec-only vma map through the focus
/// lane (the same uvma lines the softirq path produces) and marks it
/// done. No locks are held across the vma_read (the pid lookup is a
/// brief bucket-lock visit); safe to call from any task context.
pub fn dump_latched_maps_taskctx() {
    use core::fmt::Write as _;
    if !crate::trace::EVENT_SLEEPSNAP.armed() {
        return;
    }
    // Collect candidates first (cheap atomic scans); the blocking vma
    // read happens with no latch/bucket locks held.
    let mut want = [0u32; 24];
    let mut nwant = 0usize;
    let mut push_want = |tgid: u32, want: &mut [u32; 24], nwant: &mut usize| {
        if tgid == 0 || *nwant == want.len() {
            return;
        }
        if VMA_DUMPED_DONE
            .iter()
            .any(|d| d.load(core::sync::atomic::Ordering::Relaxed) == tgid)
        {
            return;
        }
        if want.iter().take(*nwant).any(|&w| w == tgid) {
            return;
        }
        want[*nwant] = tgid;
        *nwant += 1;
    };
    for v in VMA_DUMPED_TGID.iter() {
        push_want(
            v.load(core::sync::atomic::Ordering::Relaxed),
            &mut want,
            &mut nwant,
        );
    }
    for v in VMA_STICKY_TGID.iter() {
        push_want(
            v.load(core::sync::atomic::Ordering::Relaxed),
            &mut want,
            &mut nwant,
        );
    }
    for &tgid in want.iter().take(nwant) {
        // 97th: respect the global uvma line budget (see its decl).
        if UVMA_LINE_BUDGET.load(core::sync::atomic::Ordering::Relaxed) == 0 {
            return;
        }
        // The latch stores thread-group ids; the leader's pid == tgid.
        let Some(tk) = crate::process::find_task_by_pid(tgid) else {
            continue;
        };
        let aspace = match tk.address_space() {
            Some(a) => a,
            None => continue,
        };
        // Blocking read: kthread context, no spinlocks held.
        let vm = aspace.vma_read();
        let mut dumped_ok = false;
        let mut outf = alloc::string::String::new();
        for v in vm.iter() {
            // sc57 (57th): the exec-only filter named code segments but
            // left the wedge's lock objects anonymous — an FFRT
            // mutexPrivate/SharedMutexPrivate found by the FUX-CHAIN
            // walk lives in a WRITABLE file-backed VMA (samgr .data/
            // .bss of the binary or a lib) or the brk heap. Emit
            // writable file-backed VMAs too (marked "W"); the heap
            // range prints as one line.
            let exec = v.flags().is_executable();
            let writable_file = !exec
                && matches!(v.vma_type(), crate::mm::vma::VmaType::FileBacked)
                && v.flags().is_writable();
            if !exec && !writable_file {
                continue;
            }
            // 97th: the taskctx dumper's per-line budget tick (see
            // UVMA_LINE_BUDGET's decl). Load-then-store, never
            // fetch_sub: at zero a fetch_sub wraps to usize::MAX and
            // the budget would never fire again. (No drop(vm) here —
            // the guard is borrowed by the iterator; returning drops
            // it at function exit.)
            let left = UVMA_LINE_BUDGET.load(core::sync::atomic::Ordering::Relaxed);
            if left == 0 {
                return;
            }
            UVMA_LINE_BUDGET.store(left - 1, core::sync::atomic::Ordering::Relaxed);
            let path = aspace
                .try_get_vma_file(v.start().as_usize())
                .map(|f| f.path())
                .unwrap_or_default();
            let _ = writeln!(
                outf,
                "uvma: tgid {} {:x}-{:x} off={:#x}{} {}",
                tgid,
                v.start().as_usize(),
                v.end().as_usize(),
                v.offset(),
                if writable_file { " W" } else { "" },
                if path.is_empty() {
                    alloc::string::String::from("-")
                } else {
                    path.chars().take(60).collect::<alloc::string::String>()
                }
            );
            dumped_ok = true;
        }
        drop(vm);
        if dumped_ok {
            crate::debug::taskdump::defer_emit_line(outf);
            // Same insert-by-value discipline as the softirq path.
            let mut dslot = (tgid as usize) % VMA_DUMPED_DONE.len();
            for (i, d) in VMA_DUMPED_DONE.iter().enumerate() {
                let cur = d.load(core::sync::atomic::Ordering::Relaxed);
                if cur == 0 || cur == tgid {
                    dslot = i;
                    break;
                }
            }
            VMA_DUMPED_DONE[dslot].store(tgid, core::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// Latch a tgid for the park-chain walk (idempotent). `sticky` writes
/// the dedicated never-evicted ring (the walk gate ORs both rings).
pub fn latch_park_walk_tgid_sticky(tgid: u32) {
    if tgid == 0 {
        return;
    }
    for v in VMA_STICKY_TGID.iter() {
        if v.load(core::sync::atomic::Ordering::Relaxed) == tgid {
            return;
        }
    }
    for v in VMA_STICKY_TGID.iter() {
        if v
            .compare_exchange(
                0,
                tgid,
                core::sync::atomic::Ordering::AcqRel,
                core::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
        {
            return;
        }
    }
    // Full: leave it — 8 wedged leaders already walked; the churned ring
    // still covers the rest.
}

/// Latch a tgid for the park-chain walk (idempotent).
pub fn latch_park_walk_tgid(tgid: u32) {
    if tgid == 0 {
        return;
    }
    if VMA_STICKY_TGID
        .iter()
        .any(|v| v.load(core::sync::atomic::Ordering::Relaxed) == tgid)
    {
        return;
    }
    for v in VMA_DUMPED_TGID.iter() {
        if v.load(core::sync::atomic::Ordering::Relaxed) == tgid {
            return;
        }
    }
    let mut slot = usize::MAX;
    for (i, v) in VMA_DUMPED_TGID.iter().enumerate() {
        if v.load(core::sync::atomic::Ordering::Relaxed) == 0 {
            slot = i;
            break;
        }
    }
    if slot == usize::MAX {
        static VMA_VICTIM: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        slot = VMA_VICTIM.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
            % VMA_DUMPED_TGID.len();
    }
    VMA_DUMPED_TGID[slot].store(tgid, core::sync::atomic::Ordering::Relaxed);
}

pub fn sleep_snapshot() {
    if !crate::trace::EVENT_SLEEPSNAP.armed() {
        return;
    }
    crate::trace::trace_sleepsnap(0, crate::process::current_pid(), 0, 0);
    // sc207 (launcher leg 13): NEVER print inside the for_each_task
    // callback - it runs under the pid-hash bucket lock, and one printk
    // per S-state task held each bucket across console port-IO for ~a
    // second in total. A timer tick landing on that CPU mid-walk runs
    // stallwatch_tick's own for_each_task at irq-exit softirq time,
    // which then spins on the very bucket the interrupted walker holds
    // (self-deadlock: the releaser is the interrupted task); the other
    // CPU's scan spins on the same bucket and the machine hard-freezes
    // with the serial cut mid-line (sc207: both vCPUs 100%, RIP in
    // pid_hash::lock_bucket from stallwatch_tick). Collect lines here,
    // print ONCE after the walk with every bucket released.
    let mut out = alloc::string::String::new();
    // 54th: high-value focus lines (ufw/uss/uvma) collect separately and
    // emit through the deferred task-context lane — the bulk census can
    // be lossy under the flood, the park-chain data cannot.
    let mut outf = alloc::string::String::new();
    // 54th ufw hunt: snapshot generation for the focus-latch persistence
    // rule (a main-stack futex wait observed in TWO DISTINCT snapshots
    // is a persistent wedge, not a startup transient).
    static UFW_GEN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let ufw_gen = UFW_GEN.fetch_add(1, core::sync::atomic::Ordering::Relaxed) & 0xff;
    use core::fmt::Write as _;
    crate::sched::hotpath::for_each_task(|task| {
        // Callback runs under the pid-hash bucket lock (same contract
        // as hung_task's check_tasks); reads are confined to the task's
        // own fields and its mapped kernel stack. oh67_poll_decode
        // still takes the raw pointer identity.
        let task_raw = task as *const crate::process::task::Task
            as *mut crate::process::task::Task;
        {
            let tk = task;
            if tk.pid() == 0 {
                return;
            }
            let state = tk.state();
            if !state.contains(crate::process::task::TaskState::INTERRUPTIBLE) {
                return;
            }
            // Stack-freed guard (KILLDUMP census race class): the walk
            // can visit a task whose exit already freed its stack.
            let Some(top) = tk.kernel_stack_top_checked() else {
                return;
            };
            // (pc, sys, a0, a1, a2) via the INV-BINDER-ENTRYFRAME helper.
            let (rip, sys, a0, a1, a2) = entry_frame(top);
            let comm = tk.comm();
            let name = core::str::from_utf8(&comm[..])
                .unwrap_or("")
                .trim_end_matches('\0');
            // 97th (hp97 toxicity): the BINDER-IDLE looper park — ioctl
            // on the binder fd with the BWR arg shape — is the resting
            // posture of every binder server thread (hundreds per
            // census round, ~1500 of hp97d's 3625 lines) and carries no
            // wedge signal: an ANOMALOUS BWR park (work pending, write
            // consumed, age) is TXN-HANG-BWR's job, not the census's.
            // Skipping the idle shape keeps the census on the
            // futex/epoll/poll parks where the notify-never-fires
            // verdict lives, at a fraction of the UART cost.
            if !(sys == 16 && a1 == 0xffffffff_c030_6201) {
                let _ = writeln!(
                    out,
                    "sleepsnap: {} tid {} state={:#x} sys={} rip={:#x} a0={:#x} a1={:#x} a2={:#x}",
                    name,
                    tk.pid(),
                    state.bits(),
                    sys,
                    rip,
                    a0,
                    a1,
                    a2
                );
            }
            // oh67: infinite-timeout poll parks are the invisible-holder
            // class (hdf_devmgr/audio_server single-fd -1 polls) — decode
            // their pollfd head + fd path into the focus (lossless) lane.
            // Budget 4 per snapshot round; reads only, print deferred.
            if sys == 7 && a2 == u64::MAX && a1 >= 1 && a0 != 0 {
                static SNAP_PFD_N: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                if SNAP_PFD_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 4 {
                    let t =
                        crate::sync::futex::oh67_poll_decode(task_raw, a0, a1, a2);
                    if !t.is_empty() {
                        let _ = writeln!(outf, "SNAPPFD tid {} ({}){}", tk.pid(), name, t);
                    }
                }
            }
            // 54th (coordinator redirect — SA:1301 stuck-init hunt): the
            // futex-census blind spot. A retry-loop futex wait (5ms
            // timeout, ETIMEDOUT, immediate re-issue) never ages into
            // FUX-STUCK's 10s threshold, and sys=202 parks skip parkstk
            // below — so a wedged handshake on a STACK mutex (the
            // distributeddata main-thread shape: futex on 0x7fffffffd004,
            // val=2, forever) prints nothing beyond uaddr. Latch up to 4
            // "focus" thread groups — any S-state task futex-parked in
            // the MAIN-STACK window — and for focus threads print the
            // USER side of the park: rbp-chain return addresses plus a
            // stack scan for code words (which image region the waiter
            // lives in), and once per latch the process's VMA map with
            // file paths, so the raw addresses symbolize offline.
            // Read contract: same as the pollfd decode below (bounded
            // page-table walks of the parker's own memory under the
            // bucket lock; hunting boots only, rux:sleepsnap (retired debug=sleepsnap)-gated).
            #[cfg(feature = "x86_64")]
            if top != 0 {
                // r10, not rcx: at syscall entry the kernel ABI moves the
                // 4th argument to r10 (syscall clobbers rcx with the
                // return address) — for futex this is the timeout ptr.
                let (urbp, ursp, ua3) = entry_frame_futex_x86(top);
                let main_stack_futex =
                    sys == 202 && a0 >= 0x7fff_ff00_0000 && a0 < 0x8000_0000_0000;
                // 61st (ohw12 tid 344): the OTHER musl-cv park class — the
                // waiter node on the thread's OWN mmap'd stack (musl
                // pthread_cond_timedwait parks on node.state inside its
                // frame, within a few KB of the entry rsp). The FFRT
                // mutexPrivate::wait BLOCK_THREAD arm parks exactly here
                // (ctx->wn.cv.wait), and the wedge's owner-of-record sat
                // in this class for 280 s without EVER getting a frame
                // walk: the park word is outside the main-stack window and
                // the tgid latch fired too late. Own-stack test: the park
                // word within 64 KB of the entry rsp (same frame region;
                // heap/condvar words and other threads' stacks are far
                // away or grow the other way).
                let own_stack_futex = sys == 202
                    && a0 >= 0x1000
                    && ursp >= 0x10000
                    && (a0.abs_diff(ursp) < 0x1_0000);
                let my_tgid = tk.tgid();
                // Direct rule (ohc35-39 lessons: first-come latch burned
                // on transients; promotion lottery lost the real gate to
                // the stuck SA pile's candidate churn). A main-stack
                // futex waiter IS the interesting event — walk its user
                // frames immediately (deduped per (tid,rip)), and dump
                // its process's EXEC-ONLY vma map ONCE per tgid (the
                // symbolization table; exec-only keeps the dump ~40-60
                // lines instead of ~160).
                // (VMA_DUMPED_TGID hoisted to module scope — see
                // latch_park_walk_tgid below.)
                // Dump-once flag ring, SEPARATE from the latch ring:
                // the txn-hang server latch inserts tgids whose threads
                // wait on THREAD stacks (samgr) — those must still get
                // their one-time map dump (ohc42 walked them mapless).
                // (56th: hoisted to module scope — the censusd task-
                // context dumper below shares the done ring.)
                {
                    let latched = VMA_DUMPED_TGID.iter().any(|v| {
                        v.load(core::sync::atomic::Ordering::Relaxed) == my_tgid
                    }) || VMA_STICKY_TGID.iter().any(|v| {
                        v.load(core::sync::atomic::Ordering::Relaxed) == my_tgid
                    });
                    let done = VMA_DUMPED_DONE.iter().any(|v| {
                        v.load(core::sync::atomic::Ordering::Relaxed) == my_tgid
                    });
                    // 97th (hp97 toxicity, round 2): the VMA map dump
                    // fired for TIMED own-stack parks too (the alive cv
                    // pools latch their tgid just by cycling) — hp97i/j
                    // printed 15.5k uvma lines (~2 MB) for hundreds of
                    // SA tgids whose maps carry no wedge evidence. Same
                    // wedge shape as the walk gate below: main-stack any
                    // timeout; own-stack/latched only when UNTIMED.
                    if my_tgid != 0
                        && (main_stack_futex || ((own_stack_futex || latched) && ua3 == 0))
                        && !done
                    {
                        // 55th: a main-stack UNTIMED park is the wedge
                        // signature — sticky-latch it so the audit churn
                        // cannot evict it before the walk-gate scan.
                        // 61st: an own-stack UNTIMED park (the musl cv
                        // waiter node class, mutexPrivate::wait's
                        // BLOCK_THREAD arm) is the same forever signature.
                        if (main_stack_futex || own_stack_futex) && ua3 == 0 {
                            latch_park_walk_tgid_sticky(my_tgid);
                        }
                        latch_park_walk_tgid(my_tgid);
                        // 61st (ohw13): cap map dumps at 2 tgids per 5 s
                        // window — the wedge mass-latch enqueued ~50
                        // dumps in one burst and flooded the deferred
                        // lane (2811 focus drops/burst), evicting the
                        // walk lines. The dump is deferred to a later
                        // burst (the done-mark stays unset); the walk
                        // below is NOT gated on this.
                        static DUMP_BURST: core::sync::atomic::AtomicU32 =
                            core::sync::atomic::AtomicU32::new(0);
                        static DUMP_N: core::sync::atomic::AtomicU32 =
                            core::sync::atomic::AtomicU32::new(0);
                        let now_j = crate::timer::get_jiffies() as u32;
                        let bid = now_j / 500;
                        if DUMP_BURST.load(core::sync::atomic::Ordering::Relaxed) != bid {
                            DUMP_BURST.store(bid, core::sync::atomic::Ordering::Relaxed);
                            DUMP_N.store(0, core::sync::atomic::Ordering::Relaxed);
                        }
                        let dump_budget = DUMP_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 2;
                        // 55th: mark done ONLY after a successful dump —
                        // ohc44 latched samgr but the first visiting thread
                        // lost the vma read (non-blocking under contention),
                        // the done mark suppressed every retry, and the
                        // whole boot walked mapless.
                        let mut dumped_ok = false;
                        if dump_budget {
                            if let Some(aspace) = tk.address_space() {
                                if let Some(vm) = aspace.vma_try_read() {
                                    for v in vm.iter() {
                                        if !v.flags().is_executable() {
                                            continue;
                                        }
                                        // 97th: global uvma line budget
                                        // (see UVMA_LINE_BUDGET's decl).
                                        let left = UVMA_LINE_BUDGET
                                            .load(core::sync::atomic::Ordering::Relaxed);
                                        if left == 0 {
                                            break;
                                        }
                                        UVMA_LINE_BUDGET.store(
                                            left - 1,
                                            core::sync::atomic::Ordering::Relaxed,
                                        );
                                        let path = aspace
                                            .try_get_vma_file(v.start().as_usize())
                                            .map(|f| f.path())
                                            .unwrap_or_default();
                                        let _ = writeln!(
                                            outf,
                                            "uvma: tgid {} {:x}-{:x} off={:#x} {}",
                                            my_tgid,
                                            v.start().as_usize(),
                                            v.end().as_usize(),
                                            v.offset(),
                                            if path.is_empty() {
                                                alloc::string::String::from("-")
                                            } else {
                                                path.chars().take(60).collect::<alloc::string::String>()
                                            }
                                        );
                                    }
                                    dumped_ok = true;
                                }
                            }
                        }
                        if dumped_ok {
                            // 55th: insert by value (first free slot, else
                            // probe). The old tgid%16 direct store let any
                            // other tgid congruent mod 16 clobber the mark
                            // (ohc45: tgid 500's map dumped 187 times in
                            // one pass, flooding the focus lane and evicting
                            // the very walk lines the lane exists for).
                            let mut dslot = (my_tgid as usize) % VMA_DUMPED_DONE.len();
                            for (i, v) in VMA_DUMPED_DONE.iter().enumerate() {
                                let cur = v.load(core::sync::atomic::Ordering::Relaxed);
                                if cur == 0 || cur == my_tgid {
                                    dslot = i;
                                    break;
                                }
                            }
                            VMA_DUMPED_DONE[dslot].store(my_tgid, core::sync::atomic::Ordering::Relaxed);
                        }
                    }
                }
                // The user frame chain, deduped per (tid, rip): a
                // parked thread re-prints only when its innermost site
                // moves (retry loops otherwise repeat the same chain
                // every 5s snapshot). Walks run for main-stack futex
                // waiters AND every thread of an already-latched tgid
                // (the ring above doubles as the latch — the full park
                // chain of the wedged process, not just its leader).
                let tgid_latched = VMA_DUMPED_TGID.iter().any(|v| {
                    v.load(core::sync::atomic::Ordering::Relaxed) == my_tgid
                }) || VMA_STICKY_TGID.iter().any(|v| {
                    v.load(core::sync::atomic::Ordering::Relaxed) == my_tgid
                });
                // 55th: auto-latch FOREVER-class futex words (untimed
                // parks, a3==0) of latch-ring tgids into the FXRS
                // word-watch. The walks name the waiter side
                // (mutexPrivate::wait on its own condvar barrier); the
                // wake side decides the verdict: a WAKE on this word
                // that the waiter never consumes = kernel wake loss;
                // NO WAKE ever = userspace never signaled (dead holder
                // / broken notify chain). Timed parks (a3 != 0) are the
                // idle/retry classes — excluded so the 16-word ring
                // stays on the forever set.
                if tgid_latched && sys == 202 && ua3 == 0 && a0 != 0 {
                    crate::sync::futex::fxrs_word_add(a0 as usize);
                }
                // 97th (hp97 toxicity): the walks re-firing for TIMED
                // own-stack parks (the alive cv-poll pools — hundreds of
                // FFRT workers, re-walked every round) put hp97c/d/e at
                // 10-13 MB of serial (~+5 MB over the proven hp95 arm)
                // and the UART drag alone stalled 6/6 boots — the sc37/
                // sc108 lesson replayed on the census. Walk now only the
                // wedge-shaped parks: main-stack futex of ANY timeout
                // (rare, historically decisive — the 54th's chain was a
                // main-stack TIMED clockwait), and own-stack/latched
                // parks only when UNTIMED (a3==0 — the forever set; a
                // timed park that cycles is alive by construction).
                if main_stack_futex || ((own_stack_futex || tgid_latched) && ua3 == 0) {
                    // 61st (ohw12 starvation): the old 64-slot table keyed
                    // ((tid<<44)|rip&0xfff_ffff_fff) mapped EVERY thread
                    // parking at the same musl futex rip (0x3fbf116dea —
                    // the pandemic's shared park rip) to the SAME slot
                    // (tid<<44 mod 64 == 0 for all tid): the first insert
                    // won, the other hundreds were "seen_recent" forever,
                    // and the wedge owner (tid 344) never got ONE frame
                    // walk in 340 s. Direct-map one slot per TID instead
                    // (tid is system-unique; 1024 slots cover every
                    // thread a boot spawns concurrently) — the time-based
                    // rewalk then works per-thread exactly as designed.
                    static UFW_SEEN: [core::sync::atomic::AtomicU64; 1024] =
                        [const { core::sync::atomic::AtomicU64::new(0) }; 1024];
                    // 55th: time-aware dedup. ohc43/44's forever-parked
                    // threads (samgr's FFRT workers and early handlers)
                    // parked/unparked at the same rip during the healthy
                    // 0-15s window and consumed their (tid,rip) keys, so
                    // the FOREVER park at 20s+ never re-walked — the
                    // holder chain stayed invisible. Re-walk a seen key if
                    // its last walk is older than UFW_REWALK_100MS (in
                    // 100ms jiffies units, HZ=100 → ~90 s): transients do
                    // not spam, forever parks re-print every ~90 s.
                    static UFW_SEEN_T: [core::sync::atomic::AtomicU32; 1024] =
                        [const { core::sync::atomic::AtomicU32::new(0) }; 1024];
                    const UFW_REWALK_100MS: u32 = u32::MAX / 2;
                    // 97th (hp97 toxicity): effectively once per
                    // (tid,rip) per BOOT. The 55th's 90 s re-walk was
                    // for parks that START after a first walk — a new
                    // park at a different rip is a new key and walks
                    // anyway; identical keys re-printed identical frozen
                    // stacks, pure UART volume (hp97d: 2238 walk lines,
                    // ~4x the distinct-thread count).
                    let key = ((tk.pid() as u64) << 44) | (rip & 0xfff_ffff_fff);
                    let idx = (tk.pid() as usize) % UFW_SEEN.len();
                    let now100 = (crate::timer::get_jiffies() / 10) as u32;
                    let seen_idx = if UFW_SEEN[idx].load(core::sync::atomic::Ordering::Relaxed)
                        == key
                    {
                        idx
                    } else {
                        usize::MAX
                    };
                    let seen_recent = seen_idx != usize::MAX && now100
                        .wrapping_sub(UFW_SEEN_T[seen_idx].load(core::sync::atomic::Ordering::Relaxed))
                        < UFW_REWALK_100MS;
                    if !seen_recent {
                        UFW_SEEN[idx].store(key, core::sync::atomic::Ordering::Relaxed);
                        UFW_SEEN_T[idx].store(now100, core::sync::atomic::Ordering::Relaxed);
                        if let Some(aspace) = tk.address_space() {
                            let root = aspace.root_ppn();
                            // INV-BINDER-PROBE: the walk+volatile-read
                            // lives in probe_u64; this adds only the
                            // page-crossing guard (a word may not span
                            // two pages — decode is best-effort).
                            let read_u64_at = |va: u64| -> Option<u64> {
                                if va & 0xfff > 0xff8 {
                                    return None;
                                }
                                probe_u64(root, va & !0xfff)
                            };
                            let mut chain = alloc::string::String::new();
                            use core::fmt::Write as _;
                            let _ = write!(chain, "{:#x}", rip);
                            let mut rbp = urbp;
                            for _ in 0..10 {
                                if rbp < 0x1000 || rbp > 0x8000_0000_0000 || rbp & 7 != 0 {
                                    break;
                                }
                                let Some(ret) = read_u64_at(rbp + 8) else { break };
                                let Some(next) = read_u64_at(rbp) else { break };
                                if ret >= 0x1000 {
                                    let _ = write!(chain, " {:#x}", ret);
                                }
                                if next <= rbp || next - rbp > 0x20_0000 {
                                    break;
                                }
                                rbp = next;
                            }
                            let _ = writeln!(
                                outf,
                                "ufw: tgid {} tid {} sys={} a0={:#x} a3={:#x} rbp={:#x} rsp={:#x}: {}",
                                my_tgid, tk.pid(), sys, a0, ua3, urbp, ursp, chain
                            );
                            // Stack scan fallback (frame-pointer-less
                            // builds): the first code-looking words above
                            // rsp, classified against exec vmas so anon
                            // arena words do not flood the line. The vma
                            // read guard is taken ONCE (no locking per
                            // word under the bucket lock).
                            let mut scan = alloc::string::String::new();
                            let mut sp = ursp & !7;
                            let mut shown = 0;
                            let vm = aspace.vma_try_read();
                            for _ in 0..640 {
                                if shown >= 12 || sp > 0x8000_0000_0000 {
                                    break;
                                }
                                if let Some(w) = read_u64_at(sp) {
                                    let looks_code = (0x5555_0000_0000..=0x5556_0000_0000)
                                        .contains(&w)
                                        || (0x7ff0_0000_0000..=0x7ffa_0000_0000).contains(&w)
                                        || (0x3f00_0000_0000..=0x4000_0000_0000).contains(&w);
                                    if looks_code {
                                        if let Some(vm) = vm.as_ref() {
                                            if let Some(v) =
                                                vm.find(crate::mm::page::VirtAddr::new(w as usize))
                                            {
                                                if v.flags().is_executable() {
                                                    let _ = write!(scan, " {:#x}", w);
                                                    shown += 1;
                                                }
                                            }
                                        }
                                    }
                                }
                                sp += 8;
                            }
                            if !scan.is_empty() {
                                let _ = writeln!(
                                    outf,
                                    "uss: tgid {} tid {} :{}",
                                    my_tgid,
                                    tk.pid(),
                                    scan
                                );
                            }
                            // 55th: HEAP-word locals dump for futex-parked
                            // threads. mutexPrivate::lock spills the lock
                            // object pointer (`this`, a heap/arena address)
                            // near rsp; grouping waiters by these words
                            // reconstructs the userspace lock graph — the
                            // word stuck at 2 (WAIT) with no live holder
                            // names the orphaned lock. Only for the
                            // forever class (untimed futex park).
                            if sys == 202 && ua3 == 0 {
                                let mut heap = alloc::string::String::new();
                                let mut hshown = 0;
                                let mut hsp = ursp & !7;
                                for _ in 0..48 {
                                    if hshown >= 6 || hsp > 0x8000_0000_0000 {
                                        break;
                                    }
                                    if let Some(w) = read_u64_at(hsp) {
                                        let looks_heap = (0x7ff0_0000_0000..=0x7ffb_0000_0000)
                                            .contains(&w)
                                            && w & 7 == 4; // futex words are 4-aligned
                                        if looks_heap {
                                            let _ = write!(heap, " {:#x}", w);
                                            hshown += 1;
                                        }
                                    }
                                    hsp += 8;
                                }
                                if !heap.is_empty() {
                                    let _ = writeln!(
                                        outf,
                                        "uhp: tgid {} tid {} :{}",
                                        my_tgid,
                                        tk.pid(),
                                        heap
                                    );
                                }
                            }
                        }
                    }
                }
            }
            // 53rd (52nd §6a): the inner wait-object naming — walk this
            // S-state task's kernel stack from its saved switch sp and
            // symbolize the frames; this names the innermost wait (wait
            // queue, completion, SleepMutex, bio page wait) the syscall
            // number alone cannot. Internally deduped: a parked task
            // prints its parkstk line once per distinct park. Futex
            // parks (202) are skipped: the FUX census already decodes
            // them (uaddr/word/owner) in more detail than a stack walk.
            if sys != 202 {
                // poll(7): nfds<=4 — decode the pollfd array from user
                // memory (fd + events per entry) plus the timeout. A
                // parked poll's fd IS the wait object; without it the
                // park names nothing (hilogd main polls ONE fd forever).
                if sys == 7 && a1 >= 1 && a1 <= 4 {
                    if let Some(aspace) = tk.address_space() {
                        let root = aspace.root_ppn();
                        let read_u64_at = |va: u64| -> Option<u64> {
                            // INV-BINDER-PROBE (walk+volatile-read) plus
                            // the page-crossing guard; read-only decode.
                            if va & 0xfff > 0xff8 {
                                return None;
                            }
                            probe_u64(root, va)
                        };
                        let mut pfl = alloc::string::String::new();
                        use core::fmt::Write as _;
                        for i in 0..a1.min(4) {
                            let base = a0 + i * 8;
                            if let Some(w) = read_u64_at(base) {
                                let fd = (w & 0xffff_ffff) as i64;
                                let ev = (w >> 32) & 0xffff;
                                let _ = write!(pfl, " fd{}={} ev={:#x}", i, fd, ev);
                            }
                        }
                        if !pfl.is_empty() {
                            let _ = writeln!(out, "pollfd: tid {}{} to={}", tk.pid(), pfl, a2 as i64);
                        }
                    }
                }
                let stk = crate::debug::taskdump::parked_kstack_syms(tk);
                if !stk.is_empty() {
                    out.push_str(&stk);
                }
            }
        }
    });
    // Emit with every pid-hash bucket released (see the sc207 note
    // above). One printk per chunk: ring records have a length cap
    // (LOG_LINE_MAX-class), so every chunk must stay UNDER it AND end on
    // a line boundary. The old cut extended 1024 FORWARD to the next
    // newline, so a chunk holding a sleepsnap line plus the head of its
    // 53rd parkstk line exceeded the record cap and printk truncated the
    // parkstk tail silently (ohc25: every parkstk line stopped at
    // __schedule). Cut at the LAST newline at-or-before the cap instead;
    // a single line longer than the cap still hard-splits (none is).
    if !out.is_empty() {
        // One printk per LINE, and every line under the 256-byte record
        // cap: printk's ring record is a fixed 256-byte text buffer and a
        // longer format string is silently truncated there (ohc25/ohc26:
        // every multi-line 1000-byte chunk died mid-parkstk, and other
        // CPUs' records spliced into the hole). Individual parkstk
        // segments are built <=230 bytes; the pollfd/sleepsnap lines are
        // far shorter. A pathological >240-byte line still hard-splits.
        let mut rest = out.as_str();
        while !rest.is_empty() {
            let mut cut = rest.len().min(240);
            if cut < rest.len() {
                // Back off to a char boundary BEFORE slicing for rfind —
                // `rest[..cut]` with a multi-byte char straddling cut
                // panics before the ohc28 backoff below ever ran (ohc47:
                // census line + unicode comm, panic in the softirq).
                while cut > 0 && !rest.is_char_boundary(cut) {
                    cut -= 1;
                }
                if let Some(nl) = rest[..cut].rfind('\n') {
                    if nl + 1 > 0 {
                        cut = nl + 1;
                    }
                }
            }
            let (chunk, tail) = rest.split_at(cut);
            // 97th (hp97): pr_warn, not pr_info — the hunt rigs run
            // LOGLEVEL=4 (rig default; the LL7 tax is +55s) and printk
            // drops level>console BEFORE the ring, so the whole census
            // was invisible on the boots that arm rux:sleepsnap (retired debug=sleepsnap). Switch-
            // gated path; standard boots never reach here.
            // HOTPATH-PRINT: protocol-error path only (malformed/failing transaction), not the happy path
            crate::pr_warn!("{}", chunk);
            rest = tail;
        }
    }
    // Focus lines go through the deferred task-context lane (censusd):
    // a softirq printk emitter carries a 100us console-emit budget and
    // the OH flood loses these lines (ohc36: ~30% of the uvma map never
    // reached the serial; ohc37: the queue flooded before focus lines
    // were separated from the bulk).
    {
        let mut rest = outf.as_str();
        while !rest.is_empty() {
            let mut cut = rest.len().min(240);
            if cut < rest.len() {
                // ohc47: boundary backoff BEFORE the rfind slice (same
                // panic class as the bulk chunker above).
                while cut > 0 && !rest.is_char_boundary(cut) {
                    cut -= 1;
                }
                if let Some(nl) = rest[..cut].rfind('\n') {
                    if nl + 1 > 0 {
                        cut = nl + 1;
                    }
                }
            }
            let (chunk, tail) = rest.split_at(cut);
            crate::debug::taskdump::defer_emit_line(alloc::string::String::from(chunk));
            rest = tail;
        }
    }
}

/// Publish one pass entry (called where the Hung set is built, inside
/// the WORLD walk). Round-robin over 16 slots; the pass re-zeros from a
/// fresh index each round (txn_owing_pass_reset) so stale rows from a
/// shorter previous pass cannot survive.
pub fn txn_owing_publish(spid: u32, stid: u32, txn: u64) {
    use core::sync::atomic::Ordering::Relaxed;
    let n = TXN_OWING_IX.fetch_add(1, Relaxed) % 16;
    TXN_OWING_SPID[n].store(spid, Relaxed);
    TXN_OWING_STID[n].store(stid, Relaxed);
    TXN_OWING_TXN[n].store(txn, Relaxed);
}

/// Clear the snapshot at the start of each pass (the caller publishes
/// fresh entries right after, under the same WORLD hold).
pub fn txn_owing_pass_reset() {
    use core::sync::atomic::Ordering::Relaxed;
    for i in 0..16 {
        TXN_OWING_SPID[i].store(0, Relaxed);
    }
    TXN_OWING_IX.store(0, Relaxed);
}

/// lessons: no task lookups, stack reads or prints under WORLD) —
/// snapshots each hung server thread's task state and entry-frame
/// syscall/rip (same read contract as sleep_snapshot) and prints one
/// TXN-HANG line per txn. It also collects every other in-flight txn
/// (queued on any todo, or stacked sender-side elsewhere) issued from
/// the hung servers' PROCESSES, printed as TXN-HANG-OUT lines — the
/// synchronous fan-out chain the handler is stuck behind, named
/// end-to-end. Rate-capped to one burst per 5 s; bounded (8 + 8 lines);
/// zero cost when rux:txnhunt (retired debug=txnhunt) is off.
pub fn txn_hang_audit() {
    if !crate::trace::EVENT_TXNHANG.armed() {
        return;
    }
    let now = crate::timer::get_jiffies() as u32;
    // One burst per 5 s (500 jiffies at HZ=100); first call fires.
    static LAST: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    use core::sync::atomic::Ordering::Relaxed;
    let last = LAST.load(Relaxed);
    if last != 0 && now.wrapping_sub(last) < 500 {
        return;
    }
    if LAST
        .compare_exchange(last, now, Relaxed, Relaxed)
        .is_err()
    {
        return;
    }

    struct Hung {
        id: u64,
        code: u32,
        spid: u32,
        stid: u32,
        cpid: u32,
        ctid: u32,
        exec_ms: u32,
        total_ms: u32,
        // leg-36B discriminator: the hung SERVER thread's own todo
        // (length + head item) and its binder read/looper posture. A
        // thread parked in BINDER_WRITE_READ with work on ITS OWN todo
        // = lost wake (kernel-side); empty own todo + no outstanding
        // nested txn anywhere = the handler deferred the reply
        // userspace-side (image-side, e.g. an FFRT worker that never
        // completes).
        thtodo: usize,
        thtodo_head: alloc::string::String,
        looper: u32,
        wfpw: bool,
        outstanding: alloc::string::String,
    }
    struct Out {
        id: u64,
        code: u32,
        fpid: u32,
        tpid: u32,
        ttid: u32,
        queued: bool,
        ms: u32,
    }
    // Pass-3 record (39th-successor lever 3 — the FFRT holder chain):
    // a thread of a hung server's process carrying a SENDER entry on its
    // txn stack is a nested sync-IPC client parked in BINDER_WRITE_READ —
    // the futex-census BLIND SPOT (fuxstuck sees futex parks; a BWR park
    // is an ioctl). When the hung handler's FFRT mutex is held by such a
    // thread, this names it.
    struct Holder {
        pid: u32,
        tid: u32,
        txn_id: u64,
        code: u32,
        to_pid: u32,
        to_tid: u32,
        todo: usize,
        looper: u32,
    }
    let mut hung: alloc::vec::Vec<Hung> = alloc::vec::Vec::new();
    let mut outs: alloc::vec::Vec<Out> = alloc::vec::Vec::new();
    let mut holders: alloc::vec::Vec<Holder> = alloc::vec::Vec::new();
    let mut hung_pids: [u32; 4] = [0; 4];
    let mut hung_n = 0usize;
    {
        let w = world_lock();
        // SAFETY: read-only traversal under WORLD (pm() casts only reach
        // each live proc's thread/todo lists; snapshot POD only — sc89).
        {
            let world = match w.as_ref() {
                Some(world) => world,
                None => return,
            };
            fn is_hung_proc(pids: &[u32], pid: u32) -> bool {
                pids.iter().any(|&p| p == pid)
            }
            // Edge B (114th): fresh OWING generation for this pass —
            // every server-owing entry >= 1 s lands in the lock-free
            // snapshot (round-robin 16 slots) whether or not the print
            // side keeps it (the Hung set caps at 8).
            txn_owing_pass_reset();
            // Pass 1: server-side txn-stack entries executing >= 1 s.
            for p in &world.procs {
                let proc = pm(world, p);
                for t in &proc.im(world).threads {
                    for x in &t.st(world).txn_stack {
                        let pick = x.pick_j_rd(world);
                        let mine = { x.to_tid(world) } == Some(t.tid);
                        if !mine || pick == 0 {
                            continue;
                        }
                        let exec = now.wrapping_sub(pick);
                        if exec >= 100 {
                            txn_owing_publish(proc.pid, t.tid, x.id);
                        }
                        if exec >= 100 && hung.len() < 8 {
                            // leg-36B: hung-server forensics, all under
                            // WORLD (outstanding_from_tid contract).
                            use core::fmt::Write;
                            let th = t.st(world);
                            let thtodo = th.todo.len();
                            let mut thtodo_head = alloc::string::String::new();
                            if let Some(wk) = th.todo.front() {
                                match wk {
                                    Work::Transaction(xt) => {
                                        let _ = write!(
                                            thtodo_head,
                                            "txn{} code{}",
                                            xt.id, xt.code
                                        );
                                    }
                                    Work::TransactionComplete => {
                                        thtodo_head.push_str("txn-complete");
                                    }
                                    Work::ReturnError(..) => {
                                        thtodo_head.push_str("return-error");
                                    }
                                    Work::Node(n) => {
                                        let _ = write!(thtodo_head, "node{}", n.id);
                                    }
                                    Work::DeadBinder { .. } => {
                                        thtodo_head.push_str("dead-binder");
                                    }
                                    Work::ClearDeathDone { .. } => {
                                        thtodo_head.push_str("clear-death-done");
                                    }
                                }
                            }
                            let outstanding =
                                outstanding_from_tid(world, proc.pid, t.tid);
                            hung.push(Hung {
                                id: x.id,
                                code: x.code,
                                spid: proc.pid,
                                stid: t.tid,
                                cpid: x.from_proc.as_ref().map(|f| f.pid).unwrap_or(0),
                                ctid: x.from_tid.unwrap_or(0),
                                exec_ms: exec.saturating_mul(10),
                                total_ms: now.wrapping_sub(x.enq_j).saturating_mul(10),
                                thtodo,
                                thtodo_head,
                                looper: th.looper,
                                wfpw: th.waiting_for_proc_work,
                                outstanding,
                            });
                            if hung_n < 4 {
                                hung_pids[hung_n] = proc.pid;
                                hung_n += 1;
                            }
                        }
                    }
                }
            }
            if hung.is_empty() {
                return;
            }
            // Pass 2 (same guard): every in-flight txn FROM a hung
            // server's process — stacked sender-side anywhere (its
            // threads included) or still queued on any todo. That is
            // the handler's synchronous fan-out chain.
            for p in &world.procs {
                let proc = pm(world, p);
                let mut scan = |x: &BinderTxn, tpid: u32, ttid: u32| {
                    let fpid = x.from_proc.as_ref().map(|f| f.pid).unwrap_or(0);
                    if fpid == 0 || !is_hung_proc(&hung_pids[..hung_n], fpid) || outs.len() >= 8 {
                        return;
                    }
                    let pick = x.pick_j_rd(world);
                    let queued = pick == 0;
                    let ms = if queued {
                        now.wrapping_sub(x.enq_j)
                    } else {
                        now.wrapping_sub(pick)
                    };
                    outs.push(Out {
                        id: x.id,
                        code: x.code,
                        fpid,
                        tpid,
                        ttid,
                        queued,
                        ms: ms.saturating_mul(10),
                    });
                };
                for wk in &proc.im(world).todo {
                    if let Work::Transaction(x) = wk {
                        let ttid = { x.to_tid(world) }.unwrap_or(0);
                        scan(x, proc.pid, ttid);
                    }
                }
                for t in &proc.im(world).threads {
                    for wk in &t.st(world).todo {
                        if let Work::Transaction(x) = wk {
                            let ttid = { x.to_tid(world) }.unwrap_or(0);
                            scan(x, proc.pid, ttid);
                        }
                    }
                    for x in &t.st(world).txn_stack {
                        // (pass 1 already reported them).
                        if { x.to_tid(world) } == Some(t.tid) {
                        }
                        let ttid = { x.to_tid(world) }.unwrap_or(0);
                        scan(x, x.to_proc.pid, ttid);
                    }
                }
            }
            // Pass 3: collect the BWR-parked nested clients inside the
            // hung servers' processes (see the Holder record). Bounded to
            // 8 — one line each at print time.
            for p in &world.procs {
                let proc = pm(world, p);
                if !is_hung_proc(&hung_pids[..hung_n], proc.pid) {
                    continue;
                }
                'th: for t in &proc.im(world).threads {
                    for x in &t.st(world).txn_stack {
                        if { x.to_tid(world) } == Some(t.tid) {
                            continue; // server-side entry, not a client park
                        }
                        if holders.len() >= 8 {
                            break 'th;
                        }
                        let th = t.st(world);
                        holders.push(Holder {
                            pid: proc.pid,
                            tid: t.tid,
                            txn_id: x.id,
                            code: x.code,
                            to_pid: x.to_proc.pid,
                            to_tid: { x.to_tid(world) }.unwrap_or(0),
                            todo: th.todo.len(),
                            looper: th.looper,
                        });
                    }
                }
            }
        }
    }
    // WORLD released — task forensics + prints happen here (sc207).
    // 77th: samgr-threads snapshot — one line per thread of the
    // registered samgr pid, whenever any hung txn exists (the audit's
    // own 5 s cadence, one snapshot per 30 s). The 8250-load wedge's
    // mutex holder is a samgr FFRT/IPC worker parked with NO pending
    // txn (invisible to every TXN-HANG pass and — when its park is a
    // short timed wait — to the fuxstuck census); its entry-frame
    // syscall + rip names the leaf the whole load path is stuck behind.
    {
        static SNAP_AT: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let last = SNAP_AT.load(Relaxed);
        if last == 0 || now.wrapping_sub(last) >= 3000 {
            if SNAP_AT
                .compare_exchange(last, now, Relaxed, Relaxed)
                .is_ok()
            {
                let samgr = binder_sareg_samgr_pid();
                if samgr != 0 {
                    use core::fmt::Write;
                    let mut w = crate::debug::backtrace::ConsoleWriter::new();
                    let mut n = 0usize;
                    crate::sched::hotpath::for_each_task_try(|task| {
                        // SAFETY: diagnostic reads under the
                        // for_each_task contract.
                        let tk = { &*task };
                        if tk.tgid() as u32 != samgr || n >= 24 {
                            return;
                        }
                        let mut sys = 0u64;
                        let mut rip = 0u64;
                        if let Some(top) = tk.kernel_stack_top_checked() {
                            // INV-BINDER-ENTRYFRAME: (pc, sys) sub-tuple.
                            let (pc, fsys, _, _, _) = entry_frame(top);
                            sys = fsys;
                            rip = pc;
                        }
                        let _ = writeln!(
                            w,
                            "SAMGR-SNAP tid={} st={:#x} sys={} rip={:#x} comm={}",
                            tk.pid(),
                            tk.state().bits(),
                            sys,
                            rip,
                            core::str::from_utf8(tk.comm())
                                .unwrap_or("?")
                                .trim_end_matches('\0')
                        );
                        // 78th (ohw115-123): a samgr FFRT worker parked in
                        // sys=45 — x86_64 RECVFROM, first misread as brk —
                        // for minutes while holding the wedge's FFRT
                        // mutex. The stacks (ohw121/123, three workers
                        // 340/343/347): sys_recvfrom -> unix_recv_ctl ->
                        // unix_wait_round -> schedule — the beget param
                        // client's socket recv never completes. On the
                        // SECOND consecutive 30 s snapshot in recvfrom,
                        // print the kernel stack (sp-up walk, same
                        // contract as TXN-HANG-STK).
                        if sys == 45 {
                            const BRKT_N: usize = 8;
                            static BRKT_TIDS: [core::sync::atomic::AtomicU32; BRKT_N] =
                                [const {
                                    core::sync::atomic::AtomicU32::new(0)
                                }; BRKT_N];
                            static BRKT_SEEN: [core::sync::atomic::AtomicU32; BRKT_N] = [const {
                                core::sync::atomic::AtomicU32::new(0)
                            }; BRKT_N];
                            let tid = tk.pid() as u32;
                            let mut slot = usize::MAX;
                            for i in 0..BRKT_N {
                                let t = BRKT_TIDS[i].load(Relaxed);
                                if t == tid {
                                    slot = i;
                                    break;
                                }
                                if t == 0
                                    && slot == usize::MAX
                                    && BRKT_TIDS[i]
                                        .compare_exchange(
                                            0,
                                            tid,
                                            Relaxed,
                                            Relaxed,
                                        )
                                        .is_ok()
                                {
                                    slot = i;
                                }
                            }
                            if slot != usize::MAX {
                                if BRKT_SEEN[slot].fetch_add(1, Relaxed) == 1 {
                                    let _ = write!(w, "SAMGR-RECVSTUCK tid={} st={:#x}", tid, tk.state().bits());
                                    if let Some(top) = tk.kernel_stack_top_checked() {
                                        #[cfg(feature = "x86_64")]
                                        extern "C" {
                                            static __stext: u8;
                                            static __etext: u8;
                                        }
                                        #[cfg(feature = "x86_64")]
                                        let (lo, hi) = (
                                            core::ptr::addr_of!(__stext) as u64,
                                            core::ptr::addr_of!(__etext) as u64,
                                        );
                                        #[cfg(not(feature = "x86_64"))]
                                        let (lo, hi) = (0u64, u64::MAX);
                                        let base = top - crate::config::KERNEL_STACK_SIZE;
                                        let thsp = tk.thread().sp;
                                        let _ = write!(w, " sp={:#x}", thsp);
                                        if thsp as usize >= base
                                            && thsp as usize + 8 <= top
                                            && (thsp & 7) == 0
                                        {
                                            let _ = write!(w, " sp-up:");
                                            let mut shown = 0;
                                            for k in 0..640usize {
                                                let Some(v) = stack_word(
                                                    thsp as usize + k * 8,
                                                    base,
                                                    top,
                                                ) else {
                                                    break;
                                                };
                                                if v >= lo && v < hi && (v & 1) == 0 {
                                                    let _ = write!(w, " {v:#x}");
                                                    shown += 1;
                                                    if shown >= 28 {
                                                        break;
                                                    }
                                                }
                                            }
                                            if shown == 0 {
                                                let _ = write!(w, " (none)");
                                            }
                                        }
                                    }
                                    let _ = writeln!(w);
                                }
                            }
                        }
                        n += 1;
                    });
                    // 78th (ohw115): the VMA rwsem owner word for samgr's
                    // mm — tid 344 was observed stuck mid-brk (sys=45) for
                    // 60+ s; if the write lock is held by a sleeper this
                    // names the founder pid directly (WRITER pid = low 32
                    // bits; bit 1<<32 = READER-owned; bit 1<<33 =
                    // NONSPINNABLE).
                    {
                        let mut done = false;
                        crate::sched::hotpath::for_each_task_try(|task| {
                            if done {
                                return;
                            }
                            // SAFETY: diagnostic read under the for_each
                            // task contract.
                            let tk = { &*task };
                            if tk.tgid() as u32 != samgr {
                                return;
                            }
                            if let Some(aspace) = tk.address_space() {
                                let (owner, count) = aspace.vma_rwsem_diag();
                                let _ = writeln!(
                                    w,
                                    "SAMGR-VMAW owner={:#x} count={}",
                                    owner, count
                                );
                                done = true;
                            }
                        });
                        if !done {
                            let _ = writeln!(w, "SAMGR-VMAW owner=? (no aspace)");
                        }
                    }
                    let _ = writeln!(w, "SAMGR-SNAP-END printed={}", n);
                }
            }
        }
    }
    // INV-BINDER-DIAG-COMM (unsafe-slim r3 -> binder_edges edge at the
    // flip): the ONE audited raw-task comm read in the diagnostics; the
    // unsafe stayed framework-side, the closure is the typed call.
    let comm_of = |p: *mut crate::process::Task| -> &str {
        crate::r#unsafe::diag_task_comm(p)
    };
    let mut w = crate::debug::backtrace::ConsoleWriter::new();
    use core::fmt::Write as _;
    for h in &hung {
        let srv = crate::process::find_task_by_pid(h.stid);
        // Raw identity for the sched diag APIs (they take *mut Task) and
        // for the comm_of capsule; reads go through `srv.as_deref()`.
        let srv_raw = srv
            .as_deref()
            .map(|t| t as *const crate::process::task::Task as *mut crate::process::task::Task)
            .unwrap_or(core::ptr::null_mut());
        let comm_srv = comm_of(srv_raw);
        let (state, sys, rip, a0, a1) = match srv.as_deref() {
            None => (0u64, 0u64, 0u64, 0u64, 0u64),
            Some(tk) => {
                let state = tk.state().bits() as u64;
                match tk.kernel_stack_top_checked() {
                    None => (state, 0, 0, 0, 0),
                    // INV-BINDER-ENTRYFRAME: (sys, pc, a0, a1) reorder.
                    Some(top) => {
                        let (pc, sys, a0, a1, _) = entry_frame(top);
                        (state, sys, pc, a0, a1)
                    }
                }
            }
        };
        let _ = writeln!(
            w,
            "TXN-HANG txn{} code{} from pid {} tid {} ({}) to pid {} tid {} ({}): exec {} ms (total {} ms): server state={:#x} sys={} rip={:#x} a0={:#x} a1={:#x}",
            h.id,
            h.code,
            h.cpid,
            h.ctid,
            comm_of(
                crate::process::find_task_by_pid(h.ctid)
                    .map(|t| t as *mut crate::process::task::Task)
                    .unwrap_or(core::ptr::null_mut()),
            ),
            h.spid,
            h.stid,
            comm_srv,
            h.exec_ms,
            h.total_ms,
            state,
            sys,
            rip,
            a0,
            a1
        );
        // debug-migrate: the hit also records into the unified framework
        // (rux:txnhang — the event is also the family's arm switch, so
        // it is armed by construction here). The SRV/OWE/STK companion
        // lines below stay serial-only.
        crate::trace::trace_txnhang(
            h.id,
            h.code,
            h.cpid,
            h.ctid,
            h.spid,
            h.stid,
            h.exec_ms as u64,
            state,
            rip,
            a0,
        );
        // SRV-SCHED (51st session): scheduler truth for the hung server —
        // state/linkage/ownership/slot/exec-clock. Discriminates the
        // state=RUNNING hung shapes: linked-but-starved (linked=1,
        // exec frozen), phantom (RUNNING unlinked slotless), a lying
        // busy slot (slot names it, exec frozen), and a genuinely
        // running server (exec advancing between 5s censuses).
        if !srv_raw.is_null() {
            let tk = srv.as_deref().expect("srv_raw non-null implies Some");
            let linked = crate::sched::hotpath::grq_diag_cfs_linked(srv_raw);
            let onrq = tk.sched_entity().is_on_rq();
            let oncpu = tk.on_cpu();
            let claim = tk.running_on_cpu.load(core::sync::atomic::Ordering::Acquire);
            let exec_ns = tk
                .sched_entity()
                .sum_exec_runtime
                .load(core::sync::atomic::Ordering::Acquire);
            let mut slot = -1i32;
            let mut swout = -1i32;
            for c in 0..crate::config::MAX_CPUS {
                if crate::sched::hotpath::cpu_state(c).current == srv_raw {
                    slot = c as i32;
                }
                if crate::sched::hotpath::switching_out_mask(srv_raw) & (1u32 << c) != 0 {
                    swout = c as i32;
                }
            }
            // 54th: latch the hung-txn SERVER's process for the
            // sleep_snapshot park-chain walk — "reply deferred
            // userspace-side" (TXN-HANG-OWE) means the handler thread
            // parks in ITS userspace before replying; naming that park
            // (ufw/uss/uvma) is the next level of the chain.
            // 55th: FOREVER-class only (exec >= 15 s). At the wedge the
            // audit sees ~60 hung-txn servers at 1 Hz and the 16-slot
            // latch ring churned through them all — samgr (the pile's
            // root) was evicted seconds after latching and its threads
            // never walked (ohc44-46). The forever class is ~8 stable
            // tgids; they stay resident in the ring.
            if h.exec_ms >= 15000 {
                latch_park_walk_tgid(h.spid);
            }
            // Edge B (114th): per-txn first-seen exec clock — the dx
            // between bursts answers "does the debtor thread EVER get
            // scheduled again" in one line (dx=0 across bursts = the
            // owed completion never ran; this is the periodic OWING-set
            // dump's progress column). 16 slots, round-robin.
            use core::sync::atomic::Ordering::Relaxed;
            static DX_TXN: [core::sync::atomic::AtomicU64; 16] =
                [const { core::sync::atomic::AtomicU64::new(0) }; 16];
            static DX_EXEC0: [core::sync::atomic::AtomicU64; 16] =
                [const { core::sync::atomic::AtomicU64::new(0) }; 16];
            static DX_IX: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let mut dx_ms: u64 = 0;
            let mut dx_known = false;
            for i in 0..16 {
                if DX_TXN[i].load(Relaxed) == h.id {
                    dx_ms = exec_ns
                        .saturating_sub(DX_EXEC0[i].load(Relaxed))
                        / 1_000_000;
                    dx_known = true;
                    break;
                }
            }
            if !dx_known {
                let i = DX_IX.fetch_add(1, Relaxed) % 16;
                DX_TXN[i].store(h.id, Relaxed);
                DX_EXEC0[i].store(exec_ns, Relaxed);
            }
            let _ = writeln!(
                w,
                "TXN-HANG-SRV txn{} tid{} st={:#x} linked={} on_rq={} on_cpu={} claim={} cpu={} slot={} swout={} pc={:#x} exec_ms={} exec_dx_ms={}",
                h.id,
                h.stid,
                tk.state().bits(),
                match linked {
                    Some(true) => 1,
                    Some(false) => 0,
                    None => -1,
                },
                onrq as i32,
                oncpu as i32,
                claim,
                tk.ti_cpu(),
                slot,
                swout,
                tk.preempt_count() as u32,
                exec_ns / 1_000_000,
                dx_ms,
            );
        }
        // leg-36B: kernel-vs-image discriminator for the hung server
        // thread (the sc331B txn14230 shape — picked a txn, parked in
        // BINDER_WRITE_READ, no nested txn visible). thtodo>0 while the
        // thread sits in read = work pending on its OWN todo = lost
        // wake (kernel bug — the read must return it); thtodo=0 and an
        // empty outstanding list = the reply was deferred userspace-
        // side (image-side handler).
        let _ = writeln!(
            w,
            "TXN-HANG-OWE txn{} tid{} thtodo={} head=[{}] looper={:#x} wait-proc-work={} outstanding=[{}]",
            h.id,
            h.stid,
            h.thtodo,
            if h.thtodo_head.is_empty() { "-" } else { &h.thtodo_head },
            h.looper,
            h.wfpw,
            if h.outstanding.is_empty() {
                "NONE - reply deferred userspace-side"
            } else {
                &h.outstanding
            }
        );
        // lb59 (eighth successor): the 30 s CreateNewUser handler parked
        // in a PRIVATE futex retry loop (sys=202, a1=0x80, stack uaddr
        // changing every ~5 s — the waited-on producer lives in another
        // thread of the same process). When any txn's exec crosses 20 s,
        // dump EVERY task's state once: the parked locations of the
        // server's sibling threads name the leaf the handler waits for
        // (the FFRT/IPC worker-exhaustion shape shows as all workers
        // parked as CLIENTS).
        if h.exec_ms >= 20_000 {
            // Re-armable once per 600 s (the global one-shot latch spent
            // itself on an early 20 s samgr txn and went dark exactly when
            // the real installs hang formed — lb70 lesson).
            static HANG_DUMP_AT: core::sync::atomic::AtomicU64 =
                core::sync::atomic::AtomicU64::new(0);
            let now_j = crate::timer::get_jiffies();
            let last = HANG_DUMP_AT.load(core::sync::atomic::Ordering::Acquire);
            if now_j.wrapping_sub(last) >= 60_000
                && HANG_DUMP_AT
                    .compare_exchange(
                        last,
                        now_j,
                        core::sync::atomic::Ordering::AcqRel,
                        core::sync::atomic::Ordering::Acquire,
                    )
                    .is_ok()
            {
                crate::debug::taskdump::dump_all_tasks("txn-hang-20s");
                // lb66 installs hang (mkdir -> journal_stop -> blkdev_write
                // -> wait_for_used_interruptible, 441 s): capture the
                // virtio ring state at the hang moment — at_device vs
                // used-ahead-of-walker vs matched-but-undelivered splits
                // the engine (lost kick / deferred softirq / descriptor
                // window) in one line. DIRECT console (pr_err from this
                // context was dropped on lb70/lb71 — printk contention;
                // the TXN-HANG lines themselves print via ConsoleWriter).
                crate::drivers::virtio::vw_snap_direct();
            }
        }
        // Leg-28 item 2 + leg-29A: name the FILE when the hung server
        // thread is stuck in a file syscall family — the pathstash
        // entry was written by this tid's own entry path (open family:
        // sys_openat; regular-file writes: sys_write; fsync family:
        // sys_fsync — sc310's hiview handler sat in fdatasync 43 min
        // with only the raw fd visible).
        // Family must MATCH the stash op: a writev to a pipe/socket
        // never re-stashes (pipe fast path), so the slot would still
        // hold an older open's path — printing it against a writev
        // entry frame mislabels the file (seen on sc314A wave-1).
        #[cfg(feature = "x86_64")]
        let fam = if matches!(sys, 2 | 257 | 437) {
            Some(crate::debug::pathstash::OP_OPEN)
        } else if matches!(sys, 1 | 20) {
            Some(crate::debug::pathstash::OP_WRITE)
        } else if matches!(sys, 74 | 75) {
            Some(crate::debug::pathstash::OP_SYNC)
        } else if matches!(sys, 77 | 76) {
            // x86_64 ABI: 77 ftruncate, 76 truncate (leg-36B).
            Some(crate::debug::pathstash::OP_TRUNC)
        } else {
            None
        };
        #[cfg(any(feature = "riscv64", feature = "aarch64"))]
        let fam = if matches!(sys, 56 | 24) {
            Some(crate::debug::pathstash::OP_OPEN)
        } else if matches!(sys, 64 | 66) {
            Some(crate::debug::pathstash::OP_WRITE)
        } else if matches!(sys, 82 | 83) {
            Some(crate::debug::pathstash::OP_SYNC)
        } else if matches!(sys, 46 | 45) {
            // asm-generic: 46 ftruncate, 45 truncate (leg-36B).
            Some(crate::debug::pathstash::OP_TRUNC)
        } else {
            None
        };
        if let Some(want) = fam {
            if let Some((op, p)) = crate::debug::pathstash::lookup_file(h.stid) {
                if op == want {
                    let opn = match op {
                        crate::debug::pathstash::OP_OPEN => "open",
                        crate::debug::pathstash::OP_WRITE => "write",
                        crate::debug::pathstash::OP_SYNC => "sync",
                        crate::debug::pathstash::OP_TRUNC => "truncate",
                        _ => "?",
                    };
                    let _ = writeln!(
                        w,
                        "TXN-HANG txn{} file-op={} path='{}'",
                        h.id, opn, p
                    );
                }
            }
        }
        // Leg-29A stuck-site probe: kernel-text words off the hung
        // thread's kernel stack, so the serial log names WHERE in the
        // kernel the handler is parked (sc310's RUNNING 43-min
        // fdatasync vs sc303's D-state open need the stuck function,
        // not just the syscall). Two windows:
        //   sp-up   from thread().sp upward — the exact parked chain
        //           when the thread SLEEPS (context-switch-fresh sp;
        //           same contract as khungtaskd's HUNG-STK).
        //   top-dn  from just under the entry pt_regs downward — the
        //           newest frames when the thread is RUNNING on
        //           another CPU (thread().sp is then stale; the live
        //           frames sit between the entry frame and the current
        //           sp, so scanning from the top catches them plus
        //           dead-frame noise).
        // Cross-CPU reads are diagnostic-only (sc261 sleep_snapshot
        // contract); symbolize offline with addr2line.
        if let Some(tk) = srv.as_deref() {
            if let Some(top) = tk.kernel_stack_top_checked() {
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
                let is_text = |v: u64| v >= lo && v < hi && (v & 1) == 0;
                let base = top - crate::config::KERNEL_STACK_SIZE;
                let thsp = tk.thread().sp;
                let mut line = alloc::string::String::new();
                // leg-33B: continuation record for the deepened sp-up
                // window (words 9-24) — printk records cap at 256 bytes.
                let mut extra = alloc::string::String::new();
                // Window (a): parked-chain words upward from sp.
                if thsp as usize >= base
                    && thsp as usize + 8 <= top
                    && (thsp & 7) == 0
                {
                let mut shown = 0;
                let _ = write!(line, " sp-up:");
                // 640-word window, leg-33B deepened: the sc329B 8-word
                // capture stopped inside the scheduler plumbing above
                // the saved sp (context_switch / irq_restore) and never
                // reached the wait-site frames of deep fs waits
                // (journal commit, bio completion). Capture up to 24
                // kernel-text words and split across a second record —
                // printk records cap at 256 bytes.
                for k in 0..640usize {
                    // INV-BINDER-STACKWIN: per-word window bounds.
                    let Some(v) = stack_word(thsp as usize + k * 8, base, top) else {
                        break;
                    };
                    if is_text(v) {
                        if shown < 8 {
                            let _ = write!(line, " {v:#x}");
                        } else {
                            let _ = write!(extra, " {v:#x}");
                        }
                        shown += 1;
                        if shown >= 24 {
                            break;
                        }
                    }
                }
                if shown == 0 {
                    line.truncate(0);
                    extra.truncate(0);
                }
                }
                // Window (b): newest frames downward from the entry frame.
                #[cfg(feature = "x86_64")]
                let entry_sz = crate::arch::x86_64::pt_regs::PT_REGS_SIZE;
                #[cfg(feature = "riscv64")]
                let entry_sz = crate::arch::riscv64::pt_regs::PT_REGS_SIZE;
                #[cfg(feature = "aarch64")]
                let entry_sz = crate::arch::aarch64::pt_regs::PT_REGS_SIZE;
                let start = (top - entry_sz).saturating_sub(8) & !7usize;
                let mut bline = alloc::string::String::new();
                let _ = write!(bline, " top-dn:");
                let mut shown = 0;
                let mut k = 0usize;
                while shown < 10 && k < 0x600 / 8 {
                    let addr = start - k * 8;
                    if addr < base + 8 {
                        break;
                    }
                    // INV-BINDER-STACKWIN: per-word window bounds.
                    let Some(v) = stack_word(addr, base, top) else {
                        break;
                    };
                    if is_text(v) {
                        let _ = write!(bline, " {v:#x}");
                        shown += 1;
                    }
                    k += 1;
                }
                if shown > 0 {
                    let _ = writeln!(
                        w,
                        "TXN-HANG-STK txn{} tid{} state={:#x} thsp={:#x}{}{}",
                        h.id,
                        h.stid,
                        state,
                        thsp,
                        line,
                        bline
                    );
                    if !extra.is_empty() {
                        let _ = writeln!(w, "TXN-HANG-STK2 txn{}{}", h.id, extra);
                    }
                }
            }
        }
    }
    for o in &outs {
        let _ = writeln!(
            w,
            "TXN-HANG-OUT txn{} code{} from pid {} -> pid {} tid {}: {} {} ms",
            o.id,
            o.code,
            o.fpid,
            o.tpid,
            o.ttid,
            if o.queued { "queue" } else { "exec" },
            o.ms
        );
    }
    // Pass-3 prints (the FFRT holder chain): one line per BWR-parked
    // nested client inside a hung server's process, WITH its current
    // syscall posture (entry-frame read — same contract as the server
    // line above; sys=16 ioctl BWR confirms the park, sys=202 futex
    // means it moved on). These threads are the futex-census blind
    // spot: if the hung handler's FFRT mutex is held by one of them,
    // the chain is handler -> mutex -> holder -> nested BWR -> peer.
    for hl in &holders {
        let t = crate::process::find_task_by_pid(hl.tid);
        let t_raw = t
            .as_deref()
            .map(|tk| tk as *const crate::process::task::Task as *mut crate::process::task::Task)
            .unwrap_or(core::ptr::null_mut());
        let (state, sys, a0, a1) = match t.as_deref() {
            None => (0u64, 0u64, 0u64, 0u64),
            Some(tk) => {
                let state = tk.state().bits() as u64;
                match tk.kernel_stack_top_checked() {
                    None => (state, 0, 0, 0),
                    // INV-BINDER-ENTRYFRAME: (sys, a0, a1) sub-tuple.
                    Some(top) => {
                        let (_, sys, a0, a1, _) = entry_frame(top);
                        (state, sys, a0, a1)
                    }
                }
            }
        };
        let _ = writeln!(
            w,
            "TXN-HANG-HLD pid {} tid {} ({}) holds-pending txn{} code{} -> pid {} tid {}: state={:#x} sys={} a0={:#x} a1={:#x} todo={} looper={:#x}",
            hl.pid,
            hl.tid,
            comm_of(t_raw),
            hl.txn_id,
            hl.code,
            hl.to_pid,
            hl.to_tid,
            state,
            sys,
            a0,
            a1,
            hl.todo,
            hl.looper
        );
    }
}

/// BC command decoder for TXN-HANG-BWR (holder-chain lane): names the
/// write-side sub-command of a parked BINDER_WRITE_READ, so a holder
/// parked mid-critical-section is attributed to the exact binder
/// operation it went to sleep on (e.g. BC_REQUEST_DEATH_NOTIFICATION —
/// samgr's SubscribeSystemAbility holds listenerMapLock_ across
/// AddDeathRecipient).
pub fn bc_name(cmd: u32) -> &'static str {
    match cmd {
        BC_TRANSACTION => "BC_TRANSACTION",
        BC_REPLY => "BC_REPLY",
        BC_TRANSACTION_SG => "BC_TRANSACTION_SG",
        BC_REPLY_SG => "BC_REPLY_SG",
        BC_FREE_BUFFER => "BC_FREE_BUFFER",
        BC_INCREFS => "BC_INCREFS",
        BC_ACQUIRE => "BC_ACQUIRE",
        BC_RELEASE => "BC_RELEASE",
        BC_DECREFS => "BC_DECREFS",
        BC_INCREFS_DONE => "BC_INCREFS_DONE",
        BC_ACQUIRE_DONE => "BC_ACQUIRE_DONE",
        BC_REGISTER_LOOPER => "BC_REGISTER_LOOPER",
        BC_ENTER_LOOPER => "BC_ENTER_LOOPER",
        BC_EXIT_LOOPER => "BC_EXIT_LOOPER",
        BC_REQUEST_DEATH_NOTIFICATION => "BC_REQUEST_DEATH_NOTIFICATION",
        BC_CLEAR_DEATH_NOTIFICATION => "BC_CLEAR_DEATH_NOTIFICATION",
        BC_DEAD_BINDER_DONE => "BC_DEAD_BINDER_DONE",
        _ => "BC_?",
    }
}

/// rux:txnhunt (retired debug=txnhunt) extension (holder-chain lane): BWR-park audit. The FFRT
/// mass-park wedge face (480 census rows in ffrt::mutexPrivate::wait,
/// libffrt 0xce0c7) names the WAITERS, never the HOLDER — the holder is
/// itself asleep mid-critical-section, and the two park sites it can be
/// asleep at are (a) another mutexPrivate of the same family (visible
/// to the futex census) or (b) a binder ioctl — invisible to the futex
/// census, THE blind spot. This audit walks every binder thread at the
/// ~1 Hz heartbeat (self-gated, one burst per 5 s) and prints, for
/// every thread whose transaction stack is non-empty (age >= 1 s) or
/// whose park shape is anomalous, one TXN-HANG-BWR line carrying:
///   - the thread's CURRENT syscall + ioctl command + the decoded
///     write-side sub-command of the very BWR it is parked in
///     (write_size/write_consumed/read_size + first pending BC cmd;
///     r41v6: read from the kernel-side ThreadState::live_bwr shadow
///     recorded at ioctl entry — the audit's timer-heartbeat context
///     cannot dereference the audited thread's user bwr, and doing so
///     through the interrupted task's page tables produced false H2
///     rows on r41v4/r41v5),
///   - its transaction-stack top with the outgoing-sync tag — WHO the
///     nested call is TO (to pid/tid, code, queued-vs-exec, age).
/// Holder shapes it discriminates:
///   H1 park=IOCTL-BWR rsz>0 stack SYNC-OUT — holder parked waiting for
///      the reply of its own nested sync call (target named inline);
///   H2 park=IOCTL-BWR rsz=0 — WRITE-ONLY BWR parked: the reference
///      kernel never blocks a write-only BWR (FlushCommands shape —
///      AddDeathRecipient / one-way notify inside listenerMapLock_), so
///      this shape is a kernel-side park bug until proven otherwise;
///   H3 park=FUTEX (or other syscall) with a stacked txn — the holder
///      is asleep in the SAME mutexPrivate family while owing a reply

///      (cross-lock wait; the mutex it wants is named by the futex
///      census, the reply it owes by the stack entry here);
///   idle pool (empty stack, waiting-for-proc-work) — counted, not
///      printed. Snapshot under WORLD (sc89: no task lookups / user
///      reads / prints under the guard); task forensics + user BWR
///      decode + prints happen after the guard drops (sc207). Bounded
///     (24 lines + 1 census line); zero cost when rux:txnhunt (retired debug=txnhunt) is off.
pub fn bwr_park_audit() {
    if !crate::trace::EVENT_TXNHANG.armed() {
        return;
    }
    let now = crate::timer::get_jiffies() as u32;
    // One burst per 5 s (500 jiffies at HZ=100), offset from the
    // TXN-HANG burst by construction (they share the heartbeat slot but
    // independent gates; worst case both fire in the same tick).
    static LAST: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    use core::sync::atomic::Ordering::Relaxed;
    let last = LAST.load(Relaxed);
    if last != 0 && now.wrapping_sub(last) < 500 {
        return;
    }
    if LAST
        .compare_exchange(last, now, Relaxed, Relaxed)
        .is_err()
    {
        return;
    }

    // Per-arch syscall numbers + entry-frame register slots for the
    // current-syscall decode (same contract as the TXN-HANG sys= read).
    #[cfg(feature = "x86_64")]
    const SYS_IOCTL: u64 = 16;
    #[cfg(feature = "riscv64")]
    const SYS_IOCTL: u64 = 29;
    #[cfg(feature = "aarch64")]
    const SYS_IOCTL: u64 = 29;
    #[cfg(feature = "x86_64")]
    const SYS_FUTEX: u64 = 202;
    #[cfg(any(feature = "riscv64", feature = "aarch64"))]
    const SYS_FUTEX: u64 = 98;

    struct Snap {
        pid: u32,
        tid: u32,
        looper: u32,
        wfpw: bool,
        todo_len: usize,
        todo_head: alloc::string::String,
        /// (id, code, from_pid, from_tid, to_pid, to_tid, one_way,
        /// outgoing_sync, pick_j, enq_j) per stacked txn.
        stack: alloc::vec::Vec<(u64, u32, u32, u32, u32, u32, bool, bool, u32, u32)>,
        /// Most recent EV_BWR ring entry (jiffies, first write cmd,
        /// write_size) — the last BWR the thread submitted, fallback
        /// attribution when the live user bwr cannot be read.
        last_bwr: Option<(u32, u32, u32)>,
        /// Live BWR ioctl shadow (r41v6): (write_size, write_consumed,
        /// read_size, first pending write cmd) recorded kernel-side at
        /// ioctl entry — the timer-context user-memory decode it
        /// replaced read through an arbitrary interrupted task's page
        /// tables (false H2 rows on r41v4/r41v5).
        live_bwr: Option<(u64, u64, u64, u32)>,
        /// Jiffies the thread entered its blocking read wait (0 = not
        /// waiting); atomic read, safe without WORLD.
        read_wait_since: u64,
    }
    let mut snaps: alloc::vec::Vec<Snap> = alloc::vec::Vec::new();
    let mut threads_n = 0usize;
    let mut procs_n = 0usize;
    {
        let w = world_lock();
        // SAFETY: read-only traversal under WORLD; POD copies and
        // bounded Strings only (sc89 contract).
        {
            let world = match w.as_ref() {
                Some(world) => world,
                None => return,
            };
            for p in &world.procs {
                let proc = pm(world, p);
                procs_n += 1;
                for t in &proc.im(world).threads {
                    threads_n += 1;
                    let th = t.st(world);
                    let mut todo_head = alloc::string::String::new();
                    if let Some(wk) = th.todo.front() {
                        match wk {
                            Work::Transaction(xt) => {
                                use core::fmt::Write;
                                let _ = write!(todo_head, "txn{} code{}", xt.id, xt.code);
                            }
                            Work::TransactionComplete => todo_head.push_str("txn-complete"),
                            Work::ReturnError(..) => todo_head.push_str("return-error"),
                            Work::Node(n) => {
                                use core::fmt::Write;
                                let _ = write!(todo_head, "node{}", n.id);
                            }
                            Work::DeadBinder { .. } => todo_head.push_str("dead-binder"),
                            Work::ClearDeathDone { .. } => todo_head.push_str("clear-death-done"),
                        }
                    }
                    let mut stack = alloc::vec::Vec::new();
                    for x in &th.txn_stack {
                        let to_tid = { x.to_tid(world) }.unwrap_or(0);
                        let one_way = x.flags & TF_ONE_WAY != 0;
                        let outgoing_sync = !one_way
                            && x.from_tid == Some(t.tid)
                            && x.from_proc.as_ref().map(|f| f.pid) == Some(proc.pid)
                            && x.to_proc.pid != proc.pid;
                        stack.push((
                            x.id,
                            x.code,
                            x.from_proc.as_ref().map(|f| f.pid).unwrap_or(0),
                            x.from_tid.unwrap_or(0),
                            x.to_proc.pid,
                            to_tid,
                            one_way,
                            outgoing_sync,
                            // SAFETY: pick_j read under WORLD.
                            x.pick_j_rd(world),
                            x.enq_j,
                        ));
                    }
                    // Most recent EV_BWR ring entry (ring is written
                    // under WORLD only; hcur is the next slot).
                    let mut last_bwr = None;
                    for k in 0..th.hist.len() {
                        let e = &th.hist[(th.hcur + th.hist.len() - 1 - k) % th.hist.len()];
                        if e.ev == EV_BWR && e.j != 0 {
                            last_bwr = Some((e.j, e.a, e.b));
                            break;
                        }
                    }
                    snaps.push(Snap {
                        pid: proc.pid,
                        tid: t.tid,
                        looper: th.looper,
                        wfpw: th.waiting_for_proc_work,
                        todo_len: th.todo.len(),
                        todo_head,
                        stack,
                        last_bwr,
                        live_bwr: th.live_bwr,
                        read_wait_since: t.read_wait_since.load(Relaxed),
                    });
                }
            }
        }
    }
    // WORLD released — task forensics + user BWR decode + prints.
    // (comm_of capsule contract: INV-BINDER-DIAG-COMM; entry-frame read
    // contract: INV-BINDER-ENTRYFRAME.)
    let comm_of = |p: *mut crate::process::Task| -> &str {
        crate::r#unsafe::diag_task_comm(p)
    };
    // Entry-frame (syscall nr, a0, a1, a2) of a task, if its kernel
    // stack is pinned and mapped.
    let entry_regs = |p: Option<&crate::process::task::Task>| -> Option<(u64, u64, u64, u64)> {
        let top = p?.kernel_stack_top_checked()?;
        // INV-BINDER-ENTRYFRAME: (sys, a0, a1, a2) reorder.
        let (_, sys, a0, a1, a2) = entry_frame(top);
        Some((sys, a0, a1, a2))
    };
    let mut w = crate::debug::backtrace::ConsoleWriter::new();
    use core::fmt::Write as _;
    let mut printed = 0usize;
    let mut idle_readers = 0usize;
    let mut write_only_parked = 0usize;
    let mut stack_holders = 0usize;
    let mut futex_parked_stack = 0usize;
    // Empty-stack futex parkers: binder-registered threads asleep in a
    // futex with nothing owed — the mutexPrivate WAITER face (already
    // named by the userspace futex census; kept separate from idle).
    let mut futex_waiters = 0usize;
    for s in &snaps {
        // Newest stack entry age (jiffies) — the >= 1 s gate shared
        // with TXN-HANG pass 1, so transient in-flight sync calls do
        // not spam.
        let stack_age = |s: &Snap| -> u32 {
            s.stack
                .iter()
                .map(|e| {
                    let stamp = if e.8 != 0 { e.8 } else { e.9 };
                    now.wrapping_sub(stamp)
                })
                .max()
                .unwrap_or(0)
        };
        let task = crate::process::find_task_by_pid(s.tid);
        // Raw identity for the *-mut-taking diag helpers below; reads go
        // through task.as_deref().
        let task_raw = task
            .as_deref()
            .map(|t| t as *const crate::process::task::Task as *mut crate::process::task::Task)
            .unwrap_or(core::ptr::null_mut());
        // State bits (RUNNING=0, INT=1, UNINT=2): a RUNNING thread
        // caught mid-ioctl must not read as parked.
        let st_bits = task
            .as_deref()
            .map(|t| t.state().bits())
            .unwrap_or(0xFFFF_FFFFu32);
        let (sys, a0, a1, _a2) = match entry_regs(task.as_deref()) {
            Some(r) => r,
            None => (0, 0, 0, 0),
        };
        // x86_64 ioctl nr sign-extends in the 64-bit register (r41v3
        // shows 0xffffffffc0306201 vs the constant 0xc0306201) —
        // compare through u32 so the BWR decode path fires.
        let in_bwr_ioctl = sys == SYS_IOCTL && (a1 as u32 as u64) == BINDER_WRITE_READ as u64;
        // Decode the live BWR the thread is parked in (if any) from the
        // KERNEL-SIDE shadow recorded at ioctl entry (r41v6). The old
        // decode dereferenced the audited thread's user bwr pointer
        // through whatever page tables the timer heartbeat interrupted —
        // same-VA zero pages of an unrelated task decoded as "w=0/0
        // rsz=0" (false write-only/H2 rows), unrelated mappings as
        // garbage w= words, unmapped VAs as "unreadable".
        let mut bwr_txt = alloc::string::String::new();
        let mut h2_write_only = false;
        if in_bwr_ioctl {
            match s.live_bwr {
                Some((wsz, wcons, rsz, first)) => {
                    let pending = wsz.saturating_sub(wcons);
                    let _ = write!(
                        bwr_txt,
                        " bwr[w={}/{} rsz={} wcmd={:#x}{}]",
                        wsz - pending,
                        wsz,
                        rsz,
                        first,
                        if first != 0xFFFF_FFFF {
                            alloc::format!("({})", bc_name(first))
                        } else {
                            alloc::string::String::new()
                        }
                    );
                    // H2 discriminator: write-only BWR, fully consumed
                    // write, still parked — the reference kernel returns
                    // from this ioctl without ever sleeping.
                    h2_write_only = rsz == 0 && pending == 0;
                    if h2_write_only {
                        write_only_parked += 1;
                    }
                }
                None => bwr_txt.push_str(" bwr[none]"),
            }
        }
        // Last-BWR fallback attribution from the hist ring.
        let last_bwr_txt = match s.last_bwr {
            Some((j, cmd, wsz)) => alloc::format!(
                " lastbwr={}ms wcmd={:#x}({})",
                now.wrapping_sub(j).saturating_mul(10),
                cmd,
                bc_name(cmd)
            ),
            None => alloc::string::String::new(),
        };
        // oh67: pollfd decode for poll-class rows (the infinite-timeout
        // single-fd parks are the invisible-holder candidates; the wide
        // catches name their head). Budget 4 rows per burst.
        let pfd_txt = {
            static PFD_N: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            static PFD_BURST: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let burst_id = now / 500;
            let prev = PFD_BURST.load(core::sync::atomic::Ordering::Relaxed);
            if burst_id != prev {
                PFD_BURST.store(burst_id, core::sync::atomic::Ordering::Relaxed);
                PFD_N.store(0, core::sync::atomic::Ordering::Relaxed);
            }
            if sys == 7
                && a1 >= 1
                && !task_raw.is_null()
                && PFD_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 4
            {
                crate::sync::futex::oh67_poll_decode(task_raw, a0 as u64, a1 as u64, _a2)
            } else {
                alloc::string::String::new()
            }
        };
        let age = stack_age(s);
        let stack_nonempty = !s.stack.is_empty();
        // 62nd: the fuxwatch owner-of-record row — force-printed regardless
        // of the filters below. The M2 freeze's last acquirer (latched in
        // fuxwatch_log_write on every 0->1 write) parks in a shape the
        // census previously skipped (own-stack BLOCK_THREAD node, no chain
        // hit): tid344's park was invisible for 20 boots while it WAS in
        // the snapshot set the whole time.
        let owner_tid = crate::sync::futex::FUXWATCH_OWNER_TID
            .load(core::sync::atomic::Ordering::Relaxed);
        let is_owner = owner_tid != 0 && s.tid == owner_tid;
        let suspicious = (stack_nonempty && age >= 100)
            || h2_write_only
            || (in_bwr_ioctl && s.todo_len > 0)
            || is_owner;
        // Bucket first, decide printing after.
        if stack_nonempty && age >= 100 {
            stack_holders += 1;
            if !in_bwr_ioctl && sys == SYS_FUTEX {
                futex_parked_stack += 1;
            }
        }
        if !stack_nonempty && !in_bwr_ioctl && !is_owner {
            if sys == SYS_FUTEX {
                futex_waiters += 1;
            } else {
                idle_readers += 1;
            }
            continue;
        }
        if !suspicious {
            idle_readers += 1;
            continue;
        }
        // sc57: the registered samgr pid's rows never consume the
        // print budget — on a wedged boot the pandemic's peripheral
        // parks crowd out exactly the rows the chain walk needs.
        let samgr_row = s.pid == crate::binder::binder_sareg_samgr_pid();
        if printed >= 24 && !samgr_row && !is_owner {
            continue;
        }
        printed += 1;
        let park = if in_bwr_ioctl {
            "IOCTL-BWR"
        } else if sys == SYS_FUTEX {
            "FUTEX"
        } else if sys == 0 {
            "NOFRAME"
        } else {
            "OTHER"
        };
        // Stack summary: top (last) entry first — the unresolved edge
        // this thread is parked behind; tag SYNC-OUT (nested call it is
        // waiting on, WHO it is TO), SYNC-IN (reply it owes), OWAY.
        let mut stx = alloc::string::String::new();
        for e in s.stack.iter().rev().take(2) {
            let (id, code, fpid, ftid, tpid, ttid, one_way, out_sync, pick, enq) = *e;
            let (mode, age_j) = if pick != 0 {
                ("exec", now.wrapping_sub(pick))
            } else {
                ("queue", now.wrapping_sub(enq))
            };
            let tag = if out_sync {
                "SYNC-OUT"
            } else if one_way {
                "OWAY"
            } else if ttid == s.tid && tpid == s.pid {
                "SYNC-IN"
            } else {
                "SYNC"
            };
            let _ = write!(
                stx,
                " [txn{} code{} {} {}.{}/{}.{}/{} {}ms]",
                id, code, tag, fpid, ftid, tpid, ttid, mode, age_j.saturating_mul(10)
            );
        }
        let rwait = if s.read_wait_since != 0 {
            alloc::format!(
                " rwait={}ms",
                (now as u64)
                    .wrapping_sub(s.read_wait_since)
                    .saturating_mul(10)
            )
        } else {
            alloc::string::String::new()
        };
        let head_txt = if s.todo_head.is_empty() {
            alloc::string::String::new()
        } else {
            alloc::format!(" head=[{}]", s.todo_head)
        };
        // sc57 (57th): FFRT waiter-chain walk for futex-parked binder
        // threads — names the contested ffrt::mutex/shared_mutex whose
        // unlock never comes (the waiter-pool census misses waiters
        // parked after the pool fills). rux:fuxstuck (retired debug=fuxstuck) gates the walker.
        let chain_txt = if park == "FUTEX"
            && crate::trace::EVENT_FUXSTUCK.armed()
            && !task_raw.is_null()
        {
            match crate::sync::futex::ffrt_chain_walk(task_raw, a0 as usize) {
                Some((t, head)) => {
                    if let Some(h) = head {
                        // sc57: ring closed — the lock object is named;
                        // scan every same-tgid thread's stack window AND
                        // (once per boot) the stack-pool regions for the
                        // lock pointer — the hit names the holder, or a
                        // suspended coroutine's stack.
                        let holders = crate::sync::futex::ffrt_lock_holder_scan(s.pid, h);
                        let regions = crate::sync::futex::ffrt_lock_region_scan(s.pid, h);
                        // 61st (ohw19 handoff): the ONE-SHOT heap map —
                        // enumerate every ffrt::mutex in the process by
                        // the vtable signature; the non-watched stuck
                        // lock with waiters = L3 (the FFRT-internal lock
                        // the M2 holder suspended on). h is the ring
                        // head member (obj+0x10); the map wants the
                        // object base. ARMED here, RUN in the censusd
                        // kthread (a full-heap walk must not run in the
                        // timer context — ohw20's console died at 35 s
                        // mid-scan with IRQs off).
                        crate::sync::futex::HEAPMAP_ARM.compare_exchange(
                            0,
                            h.wrapping_sub(0x10),
                            core::sync::atomic::Ordering::Relaxed,
                            core::sync::atomic::Ordering::Relaxed,
                        );
                        // oh67: arm the M2 decode slot (member resolution
                        // + true-layout waiter ring + CoTask decode), run
                        // once per audit round from this same print path
                        // (page-table reads only — no heap walk). SAMGR
                        // rows only: the member table is the samgr
                        // singleton's (hdf_devmgr's binary coincidentally
                        // shares the RX-segment shape).
                        if s.pid as u32 == crate::binder::binder_sareg_samgr_pid() {
                            crate::sync::futex::OH67_M2.compare_exchange(
                                0,
                                h.wrapping_sub(0x10),
                                core::sync::atomic::Ordering::Relaxed,
                                core::sync::atomic::Ordering::Relaxed,
                            );
                            crate::sync::futex::OH67_M2_TGID.compare_exchange(
                                0,
                                s.pid as u32,
                                core::sync::atomic::Ordering::Relaxed,
                                core::sync::atomic::Ordering::Relaxed,
                            );
                        }
                        let m2txt = {
                            // Once per audit burst (the ring closes for
                            // every waiter row; one decode answers all).
                            static M2_LAST_J: core::sync::atomic::AtomicU32 =
                                core::sync::atomic::AtomicU32::new(0);
                            let lj = M2_LAST_J.load(core::sync::atomic::Ordering::Relaxed);
                            if now.wrapping_sub(lj) >= 500 {
                                M2_LAST_J.store(now, core::sync::atomic::Ordering::Relaxed);
                                crate::sync::futex::oh67_m2_decode()
                            } else {
                                alloc::string::String::new()
                            }
                        };
                        if !m2txt.is_empty() {
                            let _ = writeln!(w, "OH67-M2{}", m2txt);
                        }
                        if !holders.is_empty() || !regions.is_empty() {
                            alloc::format!(
                                " ffrt-chain[{}] ffrt-holders[{}] ffrt-regions[{}]",
                                t, holders, regions
                            )
                        } else {
                            alloc::format!(" ffrt-chain[{}]", t)
                        }
                    } else {
                        alloc::format!(" ffrt-chain[{}]", t)
                    }
                }
                None => alloc::string::String::new(),
            }
        } else {
            alloc::string::String::new()
        };
        // 61st (ohw12): OWNER-directed scan — for the H3 class (parked in
        // a futex WHILE owing/processing a transaction: the suspended
        // critical-section owner, e.g. tid 344 holding the map mutex
        // through a code=6 handler) and for the registered samgr's futex
        // parks generally, scan the parker's OWN frame spills for
        // mutexPrivate-shaped objects and print each hit's l + waiter
        // ring: the owner's next-acquisition target (M2) is named, so
        // the M2→M3 chain assembles across threads. Bounded: the scan
        // walks one page-table window; budget 3 prints per burst.
        let mscan_txt = {
            static MS_N: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            // 78th: dedicated samgr budget — first-come ordering let
            // non-samgr hung-txn rows eat all 3 scans before the walk
            // reached the samgr load-waiters, so the chain-driven
            // FUXWATCH2 arm and the live-word publish never fired. samgr
            // rows count against MS_S only.
            static MS_S: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            static MS_BURST: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            // Per-BURST budget (the audit runs one burst per 5 s): reset
            // the counter whenever the burst id moves — the first cut
            // (3 per BOOT) spent the whole budget on the first burst's
            // three H3 victims and never scanned the FFRT-worker holders
            // in later bursts.
            let burst_id = now / 500;
            let prev_burst = MS_BURST.load(core::sync::atomic::Ordering::Relaxed);
            if burst_id != prev_burst {
                MS_BURST.store(burst_id, core::sync::atomic::Ordering::Relaxed);
                MS_N.store(0, core::sync::atomic::Ordering::Relaxed);
                MS_S.store(0, core::sync::atomic::Ordering::Relaxed);
            }
            let samgr_row = s.pid as u32 == crate::binder::binder_sareg_samgr_pid()
                && crate::binder::binder_sareg_samgr_pid() != 0;
            let eligible = park == "FUTEX"
                && !task_raw.is_null()
                && (!s.stack.is_empty() || samgr_row);
            let budget_ok = if samgr_row {
                MS_S.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 3
            } else {
                MS_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 3
            };
            if (eligible && budget_ok || is_owner)
                && park == "FUTEX"
                && !task_raw.is_null()
                && crate::trace::EVENT_FUXSTUCK.armed()
            {
                match crate::sync::futex::ffrt_mutex_scan(task_raw) {
                    Some(t) => {
                        // 77th: auto-arm fuxwatch slot 2 on the live
                        // contended mutex word (chain-detected; shifts
                        // per boot). Only when the slot is free and a
                        // samgr-thread scan found it.
                        let w2 = crate::sync::futex::FRMTX_LIVE_WORD
                            .load(core::sync::atomic::Ordering::Relaxed);
                        if w2 != 0
                            && s.pid as u32
                                == crate::binder::binder_sareg_samgr_pid()
                        {
                            if crate::sync::futex::FUXWATCH2_ADDR
                                .compare_exchange(
                                    0,
                                    w2,
                                    core::sync::atomic::Ordering::Relaxed,
                                    core::sync::atomic::Ordering::Relaxed,
                                )
                                .is_ok()
                            {
                                crate::pr_debug!(
                                    "FUXWATCH2-CHAIN word={:#x} (live contended FFRT mutex)",
                                    w2
                                );
                                crate::debug::taskdump::ensure_defer_thread();
                            }
                        }
                        alloc::format!(" mscan[{}]", t)
                    }
                    None => alloc::string::String::new(),
                }
            } else {
                alloc::string::String::new()
            }
        };
        // 62nd: the owner row's dedicated payload — the deep frame chain
        // (rbp chain + spill window, offline-symbolized), the wake
        // history of its park word, the live park state (age + woken +
        // other waiters), and the PARK WORD'S OWN SHAPE (if the word is
        // an l word of a mutex the ±64MB heap map never swept — the
        // worker-pool-region classes — arm fuxwatch2 on it and dump its
        // object window). Deep + shape once per boot; wakes + park
        // state every burst.
        let owner_txt = if is_owner {
            static OWNER_DEEP_DONE: core::sync::atomic::AtomicBool =
                core::sync::atomic::AtomicBool::new(false);
            // oh67: the one-shot deep dump spent itself on the first
            // (benign, early) owner row in every boot — re-arm while the
            // latched holder has been holding >30 s, budget 3 stuck-dumps.
            static OWNER_DEEP_STUCK: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let owner_age_j = now.wrapping_sub(
                crate::sync::futex::FUXWATCH_OWNER_J
                    .load(core::sync::atomic::Ordering::Relaxed),
            );
            let stuck_dump = owner_age_j > 300
                && OWNER_DEEP_STUCK.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 3;
            let wakes = crate::sync::futex::fuxw_word_wake_summary(a0 as u64, now);
            let (page, pwoken, pothers) =
                crate::sync::futex::fuxw_owner_park_state(s.tid, a0 as u64, now);
            let mut deep = alloc::string::String::new();
            if OWNER_DEEP_DONE
                .compare_exchange(
                    false,
                    true,
                    core::sync::atomic::Ordering::Relaxed,
                    core::sync::atomic::Ordering::Relaxed,
                )
                .is_ok()
                || stuck_dump
            {
                use core::fmt::Write as _;
                let body = match crate::sync::futex::deep_ufw(task_raw) {
                    Some(t) => t,
                    None => alloc::string::String::from("none"),
                };
                let _ = write!(deep, " FUX-OWNER-DEEP[{}]", body);
                // The park word's own object shape: an l word here names
                // M3 (the inner mutex the ±64MB arena scan missed).
                match crate::sync::futex::fuxw_word_shape(task_raw, a0 as u64) {
                    Some(sh) => {
                        let _ = write!(deep, " FUX-OWNER-SHAPE[{}]", sh);
                        // Arm slot 2 on the shape-detected l word (the
                        // address of the l word = a0-0x8 for the
                        // mutexPrivate/musl-mutex layout).
                        let wl = (a0 as u64).wrapping_sub(0x8);
                        crate::sync::futex::FUXWATCH2_ADDR.compare_exchange(
                            0,
                            wl,
                            core::sync::atomic::Ordering::Relaxed,
                            core::sync::atomic::Ordering::Relaxed,
                        );
                        let _ = write!(deep, " M3-ARM l={:#x}", wl);
                    }
                    None => {
                        let _ = write!(deep, " FUX-OWNER-SHAPE[cv-word]");
                    }
                }
            }
            alloc::format!(
                " OWNER-LATCH(owner_tid={} latch_age={}ms) park[age={}ms woken={} others={}] wakes[{}]{}",
                owner_tid,
                now.wrapping_sub(
                    crate::sync::futex::FUXWATCH_OWNER_J
                        .load(core::sync::atomic::Ordering::Relaxed)
                )
                .saturating_mul(10),
                page,
                pwoken,
                pothers,
                wakes,
                deep
            )
        } else {
            alloc::string::String::new()
        };
        // 81st (ohw126 frontier): the sync-holder futex park verdict.
        // A thread parked in FUTEX while STILL holding a sync txn (the
        // AMS-looper shape: foundation tid 852 owing accountmgr's
        // txn12953) is the install-stream blocker. Latch its park word
        // (arms the write watch + the immediate wake verdict + the oh68
        // decode tgid) and print the per-burst verdict fragment:
        //   wake[] — the sc58 ring's history for THIS word (ring covers
        //     ~6 s, so the FIRST burst still sees the park's birth);
        //   park[] — the kernel waiter pool's live state (age resets =
        //     the waiter woke and re-parked; woken set but still parked =
        //     wake delivered but not consumed).
        let parkv_txt = if park == "FUTEX"
            && !s.stack.is_empty()
            // looper!=0 — a registered/entered IPC looper, the AMS-looper
            // shape (ohw127 first cut latched render_service's compositor
            // tid 785, looper=0x0, on a 2 s transient park at 38 s and
            // stole the slots).
            && s.looper != 0
            && crate::trace::EVENT_FUXSTUCK.armed()
            && a0 as u64 >= 0x1000
        {
            crate::sync::futex::fuxpark_latch(a0 as u64, s.tid, s.pid as u32);
            let wakes = crate::sync::futex::fuxw_word_wake_summary(a0 as u64, now);
            let (page, pwoken, pothers) =
                crate::sync::futex::fuxw_owner_park_state(s.tid, a0 as u64, now);
            let park_age_ms = page;
            // oh68 sweep for the park's own tgid, hoisted out of the
            // chain-walk closure (ohw130: the accountmgr tid-1188 park's
            // walk returned None — the decode never ran; the sweep's
            // task labels + ctx+224 parkw name the suspended task by
            // construction). Once per burst. hw1 (huks-wedge): per-TGID
            // gate — the single global gate let the first stalled tgid's
            // row monopolize every 5 s window (hw1w1: 73 decodes for
            // huks, zero for foundation/distributeddata whose words were
            // the frozen ones).
            if census_tgid_due(0, s.pid as u32, now, 500) {
                let t = crate::sync::futex::oh68_holder_decode_tgid(s.pid as u32);
                if !t.is_empty() {
                    let _ = writeln!(w, "OH68-PARK{}", t);
                }
            }
            // 81st: the waiter's own user stack, once the park is
            // clearly STANDING (age > 30 s) and at most once per 60 s —
            // the frames above the futex syscall name the FFRT wait
            // primitive and the handler that parked.
            if park_age_ms > 30_000 {
                if census_tgid_due(1, s.pid as u32, now, 600) {
                    if !task_raw.is_null() {
                        user_task_park_stack_w(task_raw, &mut w);
                        // 81st: if the parked word is a FROZEN rwlock
                        // (0x80000000|N-readers — ohw135 tid 1583: val
                        // 0x80000010, 16 readers never release), scan the
                        // tgid's stacks for the word address: each hit is
                        // a READER holding its critical section, and its
                        // own park word/shape prints inline.
                        // hw1 (huks-wedge): the rwlock-shape gate (bit31)
                        // hid the musl-MUTEX face of the same freeze: the
                        // futex-watch decode of hw1w1 proved the words
                        // are pthread-mutex lock words (acquire CAS
                        // musl+0x11b8d7 0->0x10, release musl+0x11bd83)
                        // and __lock words (0x2 = locked+waiters) —
                        // huks's whole -224 family parks at val 0x2 and
                        // never got the scan. Any nonzero holder count
                        // with a live waiter now qualifies; the
                        // park-standing gate above does the filtering.
                        if let Some(v) =
                            crate::sync::futex::read_park_word_via_task(task_raw, a0 as u64)
                        {
                            let readers = v & 0x7fff_ffff;
                            if readers >= 1 && readers < 0x4000_0000 {
                                crate::sync::futex::rwfrz_reader_scan(s.pid as u32, a0 as u64);
                                // 82nd: deep freeze (8+ readers) — the
                                // read-holders are yielded coroutines no
                                // thread scan can see; run the
                                // SerialQueue/QueueTask walk slice for
                                // THIS word (sliced 500k words / 5 s;
                                // latch per word).
                                if readers >= 8 {
                                    crate::sync::futex::sqwalk_step(s.pid as u32, a0 as u64);
                                }
                            }
                        }
                    }
                }
            }
            alloc::format!(
                " parkv[wake[{}] park[age={}ms woken={} others={}] jlatch={}ms]",
                wakes,
                page,
                pwoken,
                pothers,
                now.wrapping_sub(
                    crate::sync::futex::FUXPARK_J
                        .load(core::sync::atomic::Ordering::Relaxed)
                )
                .saturating_mul(10)
            )
        } else {
            alloc::string::String::new()
        };
        // 81st (ohw133): the BWR-parked twin — a looper parked in an
        // ioctl READ owing a sync reply (sensors -> pid 562 tid 713,
        // 200 s+, the ohw126 tid-766 class). The handler runs on FFRT
        // workers: the oh68 sweep of THIS tgid names them; the thread's
        // own user stack names the dispatch frames. Same budgets as the
        // futex variant.
        if park == "IOCTL-BWR"
            && !s.stack.is_empty()
            && s.looper != 0
            && s.read_wait_since != 0
            && now.wrapping_sub(s.read_wait_since as u32) > 300
            && crate::trace::EVENT_FUXSTUCK.armed()
        {
            // hw1: per-tgid gates (see census_tgid_due) — same
            // starvation class as the FUTEX twin above.
            if census_tgid_due(2, s.pid as u32, now, 500) {
                let t = crate::sync::futex::oh68_holder_decode_tgid(s.pid as u32);
                if !t.is_empty() {
                    let _ = writeln!(w, "OH68-BWRPARK{}", t);
                }
            }
            if now.wrapping_sub(s.read_wait_since as u32) > 3000
                && census_tgid_due(3, s.pid as u32, now, 600)
                && !task_raw.is_null()
            {
                user_task_park_stack_w(task_raw, &mut w);
            }
        }
        let _ = writeln!(
            w,
            "TXN-HANG-BWR pid={} tid={} ({}) park={} st={:#x} sys={} a0={:#x} a1={:#x}{}{}{} looper={:#x} wfpw={} todo={}{}{} stack={}{}{}{}{}{}",
            s.pid,
            s.tid,
            comm_of(task_raw),
            park,
            st_bits,
            sys,
            a0,
            a1,
            bwr_txt,
            last_bwr_txt,
            pfd_txt,
            s.looper,
            s.wfpw,
            s.todo_len,
            head_txt,
            rwait,
            s.stack.len(),
            stx,
            chain_txt,
            mscan_txt,
            owner_txt,
            parkv_txt
        );
    }
    if printed > 0 {
        let _ = writeln!(
            w,
            "TXN-HANG-BWR-CENSUS procs={} threads={} printed={} stack-holders={} write-only-parked={} futex-parked-stack={} futex-waiters={} idle={}",
            procs_n, threads_n, printed, stack_holders, write_only_parked, futex_parked_stack, futex_waiters, idle_readers
        );
    }
}

/// rux:ams46 (retired debug=ams46) (leg-30B): ~5 s census of the latched AMS node + its
/// foundation process. Prints ONLY while there is pending work on the
/// node/proc (async slot held, txns parked, or proc todo non-empty), so
/// a healthy quiesced draw costs zero lines. The census is the
/// adjudication backdrop for the traced code-46 txn: parked>0 with a
/// held slot names slot starvation (the holder's buffer never freed);
/// todo>0 with waiting==0 names a reader-less foundation (no eligible
/// looper thread to wake); everything zero with no READ line after ENQ
/// means the txn already vanished (retire — see the RETIRED prints).
pub fn ams46_audit() {
    if !ams46_on() {
        return;
    }
    let now = crate::timer::get_jiffies() as u32;
    // One census per 5 s (500 jiffies at HZ=100); first call fires.
    static LAST: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    use core::sync::atomic::Ordering::Relaxed;
    let last = LAST.load(Relaxed);
    if last != 0 && now.wrapping_sub(last) < 500 {
        return;
    }
    if LAST.compare_exchange(last, now, Relaxed, Relaxed).is_err() {
        return;
    }
    let latched = ams46_node_id();
    if latched == 0 {
        return;
    }
    let tpid = AMS46_TPID.load(Relaxed);
    let mut todo_d = 0usize;
    let mut waiting = 0usize;
    let mut threads_n = 0usize;
    let mut parked = 0usize;
    let mut async_held = false;
    let mut found = false;
    {
        let w = world_lock();
        // SAFETY: read-only traversal under WORLD (pm() casts only reach
        // the foundation proc's thread/node/todo lists; POD snapshot
        // only — sc89).
        {
            let world = match w.as_ref() {
                Some(world) => world,
                None => return,
            };
            if let Some(p) = world.procs.iter().find(|p| p.pid == tpid) {
                let pim = pm(world, p).im(world);
                todo_d = pim.todo.len();
                threads_n = pim.threads.len();
                waiting = pim
                    .threads
                    .iter()
                    .filter(|t| {
                        let th = t.st(world);
                        th.waiting_for_proc_work && th.todo.is_empty() && th.txn_stack.is_empty()
                    })
                    .count();
                if let Some(n) = pim.nodes.iter().find(|n| n.id == latched) {
                    let ns = n.st(world);
                    async_held = ns.has_async_transaction;
                    parked = ns.async_todo.len();
                }
                found = true;
            }
        }
    }
    if !found {
        return;
    }
    // leg-30B5 THREAD-SNAPSHOT: lb32 showed the code-46 READ by
    // foundation's main thread, its IsOsAccountExists reply returned,
    // then the thread went dark with the async slot held forever and
    // zero follow-up binder traffic — stuck in pure userspace inside
    // the StartUser handler. The reader task's in-flight syscall
    // (kernel-stack entry frame) + syscall-nr ring names the shape:
    // nanosleep poll loop (the isBoot BMS retry ladder), a futex park,
    // or live compute.
    {
        use core::sync::atomic::Ordering::Relaxed;
        let rtid = AMS46_READER.load(Relaxed);
        if rtid != 0 {
            static SNAP_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let n = SNAP_N.fetch_add(1, Relaxed);
            if n < 40 && (n % 2) == 0 {
                let tk = crate::process::find_task_by_pid(rtid);
                let t = tk.as_deref();
                if t.is_none() {
                    crate::pr_debug!("AMS46-T j={} tid={} GONE", now / 100, rtid);
                } else {
                    let t = t.expect("checked above");
                    let state = t.state().bits() as u64;
                    let (sys, rip, a0) = match t.kernel_stack_top_checked() {
                        None => (0u64, 0u64, 0u64),
                        // INV-BINDER-ENTRYFRAME: (sys, pc, a0) reorder.
                        Some(top) => {
                            let (pc, fsys, fa0, _, _) = entry_frame(top);
                            (fsys, pc, fa0)
                        }
                    };
                    use core::fmt::Write;
                    let mut s = alloc::string::String::new();
                    const RL: usize = crate::process::task::LAST_CALLS_LEN;
                    let cur = t.ext.last_calls_cur.load(Relaxed);
                    let mut shown = 0usize;
                    for k in 0..RL {
                        // newest first
                        let i = (cur + RL - k) % RL;
                        let v = t.ext.last_calls[i].load(Relaxed);
                        if v != 0 {
                            let _ = write!(s, " {}", v & 0xffff);
                            shown += 1;
                            if shown >= 16 {
                                break;
                            }
                        }
                    }
                    crate::pr_debug!(
                        "AMS46-T j={} tid={} state={:#x} sys={} rip={:#x} a0={:#x} calls:{}",
                        now / 100,
                        rtid,
                        state,
                        sys,
                        rip,
                        a0,
                        s
                    );
                }
            }
        }
    }
    if async_held || parked > 0 || todo_d > 0 {
        static CEN_N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
        if CEN_N.fetch_add(1, Relaxed) < 240 {
            crate::pr_debug!(
                "AMS46 census j={} pid={} todo={} waiting={} thr={} node{} async={} parked={}",
                now / 100,
                tpid,
                todo_d,
                waiting,
                threads_n,
                latched,
                async_held,
                parked
            );
        }
    }
}

/// POOL CENSUS (leg-33B, rux:poolcensus (retired debug=poolcensus)): every ~5 s, one line per
/// binder proc that has queued todo work — the reader-exhaustion
/// adjudication backdrop (sc329B: foundation proc todo grew while every
/// OS_IPC reader was parked as a client and the pool never grew).
/// Line shape (compact, one printk record):
///   POOL pid={} todo={} max={} req={} st={} thr={} w={} wwait={} wstk={} clients={}"
/// plus, for the proc with the deepest todo, one detail line naming each
/// worker thread's state (waiting / stacked / loitering without flags).
pub fn pool_census_audit() {
    if !pool_census_on() {
        return;
    }
    let now = crate::timer::get_jiffies() as u32;
    static LAST: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    use core::sync::atomic::Ordering::Relaxed;
    let last = LAST.load(Relaxed);
    if last != 0 && now.wrapping_sub(last) < 500 {
        return;
    }
    if LAST.compare_exchange(last, now, Relaxed, Relaxed).is_err() {
        return;
    }

    // Snapshot under WORLD: (pid, todo, max, req, thr, workers, waiting,
    // stacked, clients, worst-thread detail for the deepest-todo proc).
    struct Snap {
        pid: u32,
        todo: usize,
        max_t: i32,
        req: i32,
        started: i32,
        thr: usize,
        workers: usize,
        wwait: usize,
        wstk: usize,
        clients: usize,
    }
    let mut snaps: alloc::vec::Vec<Snap> = alloc::vec::Vec::new();
    // Deepest-todo proc's per-thread states: (tid, waiting, stacked).
    let mut detail: alloc::vec::Vec<(u32, bool, bool)> = alloc::vec::Vec::new();
    let mut detail_pid = 0u32;
    // Deepest-todo proc's first queued items (the strand victims):
    // (txn id, code, oneway, reg_tgid, sender pid, enqueue age jiffies).
    let mut todo_items: alloc::vec::Vec<(u64, u32, bool, u32, u32, u32)> =
        alloc::vec::Vec::new();
    {
        let w = world_lock();
        // SAFETY: read-only POD snapshot under WORLD (sc89 discipline —
        // no prints or task walks under the lock).
        {
            let Some(world) = w.as_ref() else { return };
            let mut deepest_pid = 0u32;
            let mut deepest_todo = 0usize;
            for p in &world.procs {
                let pim = p.im(world);
                let todo = pim.todo.len();
                if todo == 0 {
                    continue;
                }
                if todo > deepest_todo {
                    deepest_todo = todo;
                    deepest_pid = p.pid;
                }
                let mut workers = 0usize;
                let mut wwait = 0usize;
                let mut wstk = 0usize;
                let mut clients = 0usize;
                for t in pim.threads.iter() {
                    let st = t.st(world);
                    let is_worker = (st.looper & (LOOPER_ENTERED | LOOPER_REGISTERED)) != 0;
                    if is_worker {
                        workers += 1;
                        if st.waiting_for_proc_work {
                            wwait += 1;
                        }
                        if !st.txn_stack.is_empty() {
                            wstk += 1;
                        }
                    } else if !st.txn_stack.is_empty() || !st.todo.is_empty() {
                        clients += 1;
                    }
                }
                if snaps.len() < 12 {
                    snaps.push(Snap {
                        pid: p.pid,
                        todo,
                        max_t: pim.max_threads,
                        req: pim.requested_threads,
                        started: pim.requested_threads_started,
                        thr: pim.threads.len(),
                        workers,
                        wwait,
                        wstk,
                        clients,
                    });
                }
            }
            if deepest_pid != 0 {
                if let Some(p) = world.procs.iter().find(|p| p.pid == deepest_pid) {
                    let pim = p.im(world);
                    for t in pim.threads.iter().take(16) {
                        let st = t.st(world);
                        detail.push((t.tid, st.waiting_for_proc_work, !st.txn_stack.is_empty()));
                    }
                    // First queued items, with the reg_tgid read-gate
                    // verdict: a waiting daemon worker takes this item
                    // only when daemon_readable (reg 0 / daemon-owned).
                    for w in pim.todo.iter().take(3) {
                        if let Work::Transaction(t) = w {
                            let reg = t.target_node.as_ref().map_or(0, |n| n.reg_tgid);
                            todo_items.push((
                                t.id,
                                t.code,
                                t.flags & TF_ONE_WAY != 0,
                                reg,
                                t.from_proc.as_ref().map_or(0, |fp| fp.pid),
                                now.wrapping_sub(t.enq_j),
                            ));
                        }
                    }
                    detail_pid = deepest_pid;
                }
            }
        }
    }
    snaps.sort_by(|a, b| b.todo.cmp(&a.todo));
    for s in snaps.iter().take(6) {
        crate::trace::trace_poolcensus(0, s.pid, s.todo as u64, s.max_t as u64);
        crate::pr_debug!(
            "POOL pid={} todo={} max={} req={} st={} thr={} w={} wwait={} wstk={} clients={}",
            s.pid,
            s.todo,
            s.max_t,
            s.req,
            s.started,
            s.thr,
            s.workers,
            s.wwait,
            s.wstk,
            s.clients
        );
    }
    if !detail.is_empty() {
        let mut line = alloc::string::String::new();
        use core::fmt::Write as _;
        for (tid, waiting, stacked) in detail.iter() {
            let _ = write!(
                line,
                " {}{}",
                tid,
                if *waiting {
                    "W"
                } else if *stacked {
                    "S"
                } else {
                    "L"
                }
            );
        }
        crate::pr_debug!("POOL-THR pid={}{}", detail_pid, line);
    }
    for (id, code, oneway, reg, from, age) in todo_items.iter() {
        // daemon_readable: would THIS proc's own waiting worker take the
        // item (work_readable_by with the daemon tgid)? False = gated to
        // a child (reg_tgid) — only the child's threads may consume it.
        let daemon_readable = *reg == 0 || *reg == detail_pid;
        crate::pr_debug!(
            "POOL-TODO pid={} txn{} code{} {} reg={} from={} age={}j{}",
            detail_pid,
            id,
            code,
            if *oneway { "oneway" } else { "sync" },
            reg,
            from,
            age,
            if daemon_readable { "" } else { " CHILD-GATED" }
        );
    }
}


/// World-wide search for queued/stacked transactions SENT by (pid, tid) —
/// the sc74 discriminator: a SERVER-OWING thread (parked owing a reply,
/// top txn to_tid == self) either has a nested sync txn outstanding
/// somewhere (legitimate wait) or has nothing (kernel lost it / userspace
/// never sent it / state corruption). Returns a short human-readable
/// location list, empty when nothing is outstanding.
/// INV-BINDER-HELD (unsafe-slim r3): un-signed; token-checked accessors.
pub fn outstanding_from_tid(world: &World, pid: u32, tid: u32) -> alloc::string::String {
    let mut found = alloc::string::String::new();
    // Flood cap (lb310 lesson): a 60-1900-deep legal-interleave stack
    // turned each census line into a 100-230KB console write that
    // drowned the boot. Print the first 8 entries verbatim, then a
    // count summary of the rest.
    let mut shown = 0usize;
    for p in &world.procs {
        let pim = p.im(world);
        for (i, wk) in pim.todo.iter().enumerate() {
            if let Work::Transaction(t) = wk {
                if t.from_tid == Some(tid) && t.from_proc.as_ref().map(|f| f.pid) == Some(pid) {
                    use core::fmt::Write;
                    let _ = write!(
                        found,
                        " [p{}todo{} txn{} code{}]",
                        p.pid, i, t.id, t.code
                    );
                }
            }
        }
        for ct in &pim.threads {
            let cth = ct.st(world);
            for (i, wk) in cth.todo.iter().enumerate() {
                if let Work::Transaction(t) = wk {
                    if t.from_tid == Some(tid) && t.from_proc.as_ref().map(|f| f.pid) == Some(pid) {
                        use core::fmt::Write;
                        shown += 1;
                        if shown <= 8 {
                            let _ = write!(
                                found,
                                " [p{}t{}todo{} txn{} code{}]",
                                p.pid, ct.tid, i, t.id, t.code
                            );
                        }
                    }
                }
            }
            for (i, st) in cth.txn_stack.iter().enumerate() {
                if st.from_tid == Some(tid) && st.from_proc.as_ref().map(|f| f.pid) == Some(pid) {
                    use core::fmt::Write;
                    shown += 1;
                    if shown <= 8 {
                        // SAFETY: to_tid read under WORLD (same contract
                        // as the rest of this walk).
                        let tt = { st.to_tid(world) };
                        let _ = write!(
                            found,
                            " [p{}t{}stack{} txn{} code{}>{}.{}]",
                            p.pid, ct.tid, i, st.id, st.code,
                            st.to_proc.pid,
                            tt.unwrap_or(0)
                        );
                    }
                }
            }
        }
        for node in &pim.nodes {
            for t in node.st(world).async_todo.iter() {
                if t.from_tid == Some(tid) && t.from_proc.as_ref().map(|f| f.pid) == Some(pid) {
                    use core::fmt::Write;
                    shown += 1;
                    if shown <= 8 {
                        let _ = write!(found, " [p{}node{} async txn{}]", p.pid, node.id, t.id);
                    }
                }
            }
        }
    }
    if shown > 8 {
        use core::fmt::Write;
        let _ = write!(found, " +{}more", shown - 8);
    }
    found
}

// hist_snapshot/hist_line_of — STAYED framework-side (cold-four lane
// correction): binder_thread_read's read-side history print calls them
// by name from the mechanism; stall_report reaches them through the
// published path below.


/// sc77: for a SERVER-OWING holder parked in the binder read with NO
/// outstanding nested txn (the "impossible shape" — OH libipc cannot
/// legally park there), snapshot the USER stack at the park: the entry
/// frame's rsp/rip plus candidate return addresses, each resolved to its
/// covering VMA (fd + file offset) so the frames symbolize offline
/// against the image's libraries.
/// SAFETY: sc89 — reads only the holder TASK's own page tables (found
/// by t.tid, which is immutable; the Arc pins the BinderThread). Safe
/// OUTSIDE the WORLD guard and must run there: this is one of the slow
/// page-walk dumps that made the old WORLD-held stall_report a ~500 ms
/// IRQ-off window (sc82, khungtaskd death).
pub fn user_park_stack_dump(t: &Arc<BinderThread>) {
    let task = crate::process::find_task_by_pid(t.tid);
    let Some(task) = task else { return };
    user_task_park_stack(task as *mut crate::process::task::Task);
}

/// 81st: the task-level half of the park-stack dump, callable from the
/// FUTEX-park census rows directly (the tid-852/1188-class parks are
/// futex parks, not BWR parks — the BinderThread variant never ran for
/// them). Frames above the waiter's rsp name the FFRT wait call site
/// and the handler that parked (module+fileoff, offline-symbolized).
/// Writes through the caller's ConsoleWriter — pr_warn from the censusd
/// audit context is dropped (the missing FUXPARK-ARM prints class).
pub fn user_task_park_stack(task: *mut crate::process::Task) {
    let mut w = crate::debug::backtrace::ConsoleWriter::new();
    user_task_park_stack_w(task, &mut w);
}

// hw1 (huks-wedge lane): per-tgid rate gate for the census deep-dives
// (OH68-PARK / park-stack / OH68-BWR / BWR park-stack). The 81st used
// ONE global last-jiffy static per gate, so with several stalled tgids
// the first row of each window monopolized the decode (hw1w1: OH68-PARK
// ran 73x for huks, 0x for foundation/distributeddata — the tgids whose
// words were actually frozen). 8 tgid slots per gate; the interval is
// jiffies. Gates: 0=OH68-PARK, 1=park-stack (futex), 2=OH68-BWR,
// 3=park-stack (BWR).
static CENSUS_GATE_TGIDS: [core::sync::atomic::AtomicU32; 32] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 32];
static CENSUS_GATE_LASTJ: [core::sync::atomic::AtomicU32; 32] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 32];

pub fn census_tgid_due(gate: usize, tgid: u32, now: u32, interval: u32) -> bool {
    use core::sync::atomic::Ordering::Relaxed;
    if tgid == 0 || gate >= 4 {
        return false;
    }
    let base = gate * 8;
    let mut free = None;
    for i in 0..8 {
        let t = CENSUS_GATE_TGIDS[base + i].load(Relaxed);
        if t == tgid {
            let lj = CENSUS_GATE_LASTJ[base + i].load(Relaxed);
            if now.wrapping_sub(lj) >= interval {
                CENSUS_GATE_LASTJ[base + i].store(now, Relaxed);
                return true;
            }
            return false;
        }
        if t == 0 && free.is_none() {
            free = Some(i);
        }
    }
    if let Some(i) = free {
        CENSUS_GATE_TGIDS[base + i].store(tgid, Relaxed);
        CENSUS_GATE_LASTJ[base + i].store(now, Relaxed);
        return true;
    }
    false
}

pub fn user_task_park_stack_w(
    task: *mut crate::process::Task,
    w: &mut crate::debug::backtrace::ConsoleWriter,
) {
    use core::fmt::Write as _;
    if task.is_null() {
        return;
    }
    // SAFETY: non-null live task from the caller's pid lookup (contract
    // named at the call sites); the fn is the park-stack capsule.
    let tk = crate::r#unsafe::diag_task(task);
    let Some(aspace) = tk.address_space() else { return };
    let root = aspace.root_ppn();
    let read_u64_at = |va: u64| -> Option<u64> {
        // INV-BINDER-PROBE (walk+volatile-read) plus the page-crossing
        // guard; read-only decode of the holder's own user memory.
        if va & 0xfff > 0xff8 {
            return None; // crosses a page — skip rather than split
        }
        probe_u64(root, va)
    };
    // Stack-freed guard (KILLDUMP census race class): the holder may
    // have exited and freed its stack — skip, never fault.
    let Some(top) = tk.kernel_stack_top_checked() else {
        return;
    };
    // INV-BINDER-ENTRYFRAME: (sp, pc) arm.
    let (rsp, rip) = entry_frame_sp(top);
    let _ = writeln!(
        w,
        "binder-stall:   PARK-STACK tid {} rip={:#x} rsp={:#x}",
        tk.pid(),
        rip,
        rsp
    );
    // 16 candidate frame words above rsp; each that lands in a mapped
    // exec-looking VMA prints with the file offset for offline symbolize.
    let vmas = aspace.vma_read();
    for i in 0..16u64 {
        let Some(v) = read_u64_at(rsp + 8 * i) else { continue };
        // Skip obvious non-pointers (small ints, non-canonical).
        if v < 0x1000 || v > 0x7fff_ffff_ffff {
            continue;
        }
        if let Some(vm) = vmas.find(crate::mm::page::VirtAddr::new(v as usize)) {
            let off = vm.offset() + (v as usize - vm.start().as_usize());
            // Instruction bytes at the frame (Session F's byte-match method:
            // search these bytes at fileoff in the debugfs-extracted .so).
            let bytes = read_u64_at(v).unwrap_or(0);
            // 81st: the module BASENAME inline — per-boot module bases
            // shift and offline VA mapping across boots fails (ohw135's
            // tid-1583 frame sat in a vma no decode table covered).
            let name: alloc::string::String = tk
                .address_space()
                .and_then(|a| a.try_get_vma_file(vm.start().as_usize()))
                .map(|f| f.path())
                .unwrap_or_default()
                .rsplit('/')
                .next()
                .unwrap_or("-")
                .chars()
                .take(28)
                .collect();
            let _ = writeln!(
                w,
                "binder-stall:   ustack[{}]={:#x} vma={:#x}-{:#x} fd={} fileoff={:#x} mod={} bytes={:#018x}",
                i,
                v,
                vm.start().as_usize(),
                vm.end().as_usize(),
                vm.file_fd(),
                off,
                name,
                bytes
            );
        }
    }
}

/// Wait-queue census + task state for a stalled reader — the live-wake
/// discriminator: an entry flagged woken while its task still sleeps is the
/// sticky-skip lost wake (W1); woken=0 means nobody ever issued the wake.
/// sc89: pure wait-queue + scheduler reads (no binder state) — safe
/// outside the WORLD guard; returns the formatted line.
pub fn thread_census_of(t: &Arc<BinderThread>) -> alloc::string::String {
    use core::fmt::Write;
    let (total, woken) = t.wait.vw_queue_census();
    let task = crate::process::find_task_by_pid(t.tid);
    let state = task
        .as_deref()
        .map(|tk| tk.state().bits())
        .unwrap_or(0xFF);
    let verdict = if task.is_none() {
        "TASK-GONE"
    } else if woken > 0 && (state & 0x3) != 0 {
        // sleeping (INTERRUPTIBLE=1/UNINTERRUPTIBLE=2) with a woken entry
        "LOST-WAKE-SHAPE"
    } else if woken > 0 {
        "woken+runnable"
    } else {
        "never-woken"
    };
    let mut s = alloc::string::String::new();
    let _ = write!(
        s,
        "  census tid {} q={} woken={} task_state={:#x} {}",
        t.tid, total, woken, state, verdict
    );
    s
}

/// rux:schb (retired debug=schb) heartbeat counts (sc-series quiet-quiet soft-wedge hunt):
/// cheap per-beat binder summary — live proc count, threads parked in
/// the blocking binder read, and waiters older than 15s (the rekick
/// threshold; a healthy boot parks readers for 1-3s at most). Read-only
/// under WORLD; no formatting, no I/O — all output happens in schbd.
pub fn schb_beat() -> (usize, usize, usize) {
    let now = crate::timer::get_jiffies();
    let hz = crate::config::KERNEL_HZ as u64;
    let mut procs = 0usize;
    let mut waiters = 0usize;
    let mut waiters15 = 0usize;
    let w = world_lock();
    // SAFETY: read-only traversal under WORLD (pm() casts only reach
    // each live proc's thread list).
    {
        if let Some(world) = w.as_ref() {
            for p in &world.procs {
                procs += 1;
                let proc = pm(world, p);
                for t in &proc.im(world).threads {
                    let since = t.read_wait_since.load(core::sync::atomic::Ordering::Acquire);
                    if since == 0 {
                        continue;
                    }
                    waiters += 1;
                    if hz != 0 && now.saturating_sub(since) >= 15 * hz {
                        waiters15 += 1;
                    }
                }
            }
        }
    }
    (procs, waiters, waiters15)
}

/// rux:schb (retired debug=schb) wedge census: EVERY read-wait binder thread regardless of
/// age. stall_report only prints waiters older than STALL_SECS (120s);
/// the census fires at pid-plateau+60s where the wedge's waiters are
/// younger. Formats under WORLD (no I/O); the caller emits the lines via
/// ConsoleWriter after the guard drops (the sc89 stall_report
/// discipline). No stall_reported latching, no rekick side effects.
pub fn schb_waiter_census(out: &mut alloc::string::String) {
    use core::fmt::Write as _;
    let now = crate::timer::get_jiffies();
    let hz = crate::config::KERNEL_HZ as u64;
    if hz == 0 {
        return;
    }
    let w = world_lock();
    {
        if let Some(world) = w.as_ref() {
            for p in &world.procs {
                let proc = pm(world, p);
                for t in &proc.im(world).threads {
                    let since = t.read_wait_since.load(core::sync::atomic::Ordering::Acquire);
                    if since == 0 {
                        continue;
                    }
                    let age = now.saturating_sub(since) / hz;
                    let th = t.st(world);
                    let depth = th.txn_stack.len();
                    // Same discrimination as stall_report's stall lines:
                    // SERVER-OWING (parked in read owing a reply — the
                    // death-loop terminal shape) vs CLIENT-WAIT (waiting
                    // for its own reply).
                    let (shape, owed) = match th.txn_stack.last() {
                        Some(txn) if txn.to_tid(world) == Some(t.tid) => {
                            ("SERVER-OWING", txn.to_proc.pid)
                        }
                        Some(txn) => ("CLIENT-WAIT", txn.to_proc.pid),
                        None => ("NO-TXN", 0),
                    };
                    let _ = writeln!(
                        out,
                        "SC-WEDGE-BW pid={} tid={} age={}s depth={} shape={} owed_pid={} kicks={}",
                        proc.pid,
                        t.tid,
                        age,
                        depth,
                        shape,
                        owed,
                        t.rekick_count.load(core::sync::atomic::Ordering::Relaxed)
                    );
                }
            }
        }
    }
}

pub fn stall_report() {
    const STALL_SECS: u64 = 120;
    /// Re-kick threshold (heal): any binder reader parked longer than this
    /// gets an idempotent wake — if the wedge is a lost wake anywhere in
    /// the chain, the re-kick unwedges it; a legitimately-parked thread
    /// just re-checks its condition and re-parks. OH's own retry ladder
    /// runs 40 s timeouts, so a 15 s re-kick keeps chains alive without
    /// spamming healthy 1-3 s waits.
    const REKICK_SECS: u64 = 15;
    let now = crate::timer::get_jiffies();
    let hz = crate::config::KERNEL_HZ as u64;
    if hz == 0 {
        return;
    }
    // sc81: rwlock-freeze watch (samgr abilityMapLock_ terminal shape) —
    // word-delta + futex-traffic + tid-census + full-tgid park-stack dump.
    crate::sync::futex::freeze_scan(now, hz);
    let mut dump_owners: Vec<Arc<BinderProc>> = Vec::new();
    // Threads to re-kick after WORLD is released (heal + forensics event).
    let mut rekick: Vec<Arc<BinderThread>> = Vec::new();
    // Procs with pending todo: re-run the pick-one wake (heals a txn that
    // sits on a proc todo because its wake was consumed in vain).
    let mut retodo: Vec<Arc<BinderProc>> = Vec::new();
    // sc89: EVERYTHING slow — console output, page-table walks (user park
    // stacks, holder forensics), futex-graph walks — moved OUT of the
    // WORLD guard. sc82 measured the old WORLD-held dump at ~500 ms with
    // irqs off (SpinlockIrqGuard<World> drop); khungtaskd died to the
    // watchdog/softlockup side effects and the freeze scanner stopped
    // scanning. Under the guard we now only FORMAT text (fast, no I/O)
    // and snapshot POD; the one pr_warn at the end flushes it all with
    // interrupts on.
    let mut out = alloc::string::String::new();
    // Impossible-shape threads: user-stack dumps AFTER the guard.
    let mut late_parkdumps: Vec<Arc<BinderThread>> = Vec::new();
    // Owner pids for the deferred futex-waiter graph walk.
    let mut late_futex_graph: Vec<u32> = Vec::new();
    // Per-owner thread list with pre-formatted lines + hist snapshots,
    // built under the guard, printed after.
    struct OwnerDump {
        pid: u32,
        name: alloc::string::String,
        todo_depth: usize,
        lines: Vec<(alloc::string::String, bool)>, // (stall line, stack non-empty)
        hists: Vec<Option<[(u8, u32, u32, u32); BHIST]>>,
        futex_line: alloc::string::String,
        threads: Vec<Arc<BinderThread>>, // for deferred holder forensics
    }
    let mut owner_dumps: Vec<OwnerDump> = Vec::new();
    let mut retire_wakes: Vec<Arc<BinderProc>> = Vec::new();
    // leg-32B: child threads that received a retired-slot handoff txn —
    // woken in the heal block below, outside WORLD.
    let mut retire_wake_ths: Vec<Arc<BinderThread>> = Vec::new();
    {
        let w = world_lock();
        // (unsafe-slim r3) un-signed: the traversal is pure safe code now —
        // every raw deref rides the token-checked accessors (pm/im/st/hist,
        // INV-BINDER-HELD), the task lookups are process::find_task_by_pid,
        // and the phase-2 census cells carry their own audited spans.
        {
            let world = match w.as_ref() {
                Some(world) => world,
                None => return,
            };
            // Leg-19: the enq_j silence rule (leg-17's banked design) —
            // retire undelivered transactions whose receivers have been
            // silent past any plausible retry ladder. Frees the dominant
            // floor population (parked oneways, pick_j==0) and unstacks
            // the senders of parked sync calls.
            let (retired, mut rw, mut rth) = sweep_silent_txns(world);
            retire_wakes.append(&mut rw);
            retire_wake_ths.append(&mut rth);
            if retired > 0 {
                let _ = writeln!(
                    out,
                    "binder-stall: SILENT-TXN-RETIRE retired {} txn(s) this scan",
                    retired
                );
            }
            for p in &world.procs {
                let proc = pm(world, p);
                if !proc.im(world).todo.is_empty() {
                    retodo.push(p.clone());
                }
                for t in &proc.im(world).threads {
                    let since = t.read_wait_since.load(core::sync::atomic::Ordering::Acquire);
                    if since == 0 {
                        continue;
                    }
                    let age = now.saturating_sub(since);
                    if age >= REKICK_SECS * hz {
                        if !t.rekicked.swap(true, core::sync::atomic::Ordering::AcqRel) {
                            t.hist(world, EV_REKICK, (age / hz) as u32, 0);
                        }
                        let kicks = t.rekick_count.fetch_add(1, core::sync::atomic::Ordering::AcqRel);
                        if kicks >= 1 {
                            // A previous kick failed to run this task within
                            // a full scan interval — wake→run leak (sc76:
                            // REKICK@6007 ignored for 230 s on a healthy
                            // idle worker; the WAKE-STRANDED family). Dump
                            // the task's scheduler view: linked? on_cpu?
                            // pick_state? — names which stage ate the wake.
                            let task = crate::process::find_task_by_pid(t.tid);
                            let task_raw = task
                                .as_deref()
                                .map(|tk| {
                                    tk as *const crate::process::task::Task
                                        as *mut crate::process::task::Task
                                })
                                .unwrap_or(core::ptr::null_mut());
                            if !task_raw.is_null() {
                                let tk = task.as_deref().expect("task_raw non-null");
                                let linked = match crate::sched::hotpath::grq_diag_cfs_linked(task_raw) {
                                    Some(b) => {
                                        if b { "1" } else { "0" }
                                    }
                                    None => "n/a",
                                };
                                use core::fmt::Write;
                                // sc99: the leak persists through kicks —
                                // name WHICH cpu slot (if any) still owns the
                                // task (the wake_up_enqueue curr_on scan) and
                                // each CPU's idle bit: a LYING BUSY slot
                                // refuses every wake silently (R56c heals
                                // only idle-marked slots) and only the idle
                                // loop's harvester could steal it — a CPU
                                // that never idles strands the task forever.
                                let mut slots = alloc::string::String::from(" slots=");
                                for (pid, idle, cur) in
                                    crate::sched::hotpath::grq_diag_cpu_currents().iter()
                                {
                                    if *pid == u32::MAX {
                                        continue;
                                    }
                                    let _ = write!(
                                        slots,
                                        "[pid{}{}{}]",
                                        pid,
                                        if *idle { "/idle" } else { "" },
                                        if *cur == task_raw { "/OWNER" } else { "" },
                                    );
                                }
                                // Leg-19 attribution: policy/on_rq/dl + the
                                // kick outcome counters — decider between
                                // "wake linked it, pick never ran it"
                                // (wok>=1, on_rq=1) and "wake silently
                                // refused" (wno-only — R82 DL defer or an
                                // empty queue: the thread parks on a
                                // futex, not on the binder waitqueue).
                                let pol = tk.policy() as u8;
                                let onrq = match tk.policy() {
                                    crate::process::task::SchedPolicy::Fifo
                                    | crate::process::task::SchedPolicy::Rr => {
                                        tk.rt_entity().is_on_rq()
                                    }
                                    crate::process::task::SchedPolicy::Deadline => {
                                        tk.dl_entity().is_on_rq()
                                    }
                                    _ => tk.sched_entity().is_on_rq(),
                                };
                                let dlthr = tk
                                    .dl_entity()
                                    .dl_throttled
                                    .load(core::sync::atomic::Ordering::Acquire);
                                // 118th: name the victim (the census lines
                                // were anonymous tids — the pk=1 monopolist
                                // and the stranded readers both need comms).
                                let comm_bytes = tk.comm();
                                let comm_len = comm_bytes.iter().position(|&b| b == 0).unwrap_or(16);
                                let comm = core::str::from_utf8(&comm_bytes[..comm_len]).unwrap_or("?");
                                // wrl-class: adjudicate the row against the
                                // previous census sample of the SAME wait
                                // episode. exec advanced => every kick ran
                                // the task (benign re-park of an idle
                                // reader); exec frozen with kicks >= 2 =>
                                // the true wake->run leak this line was
                                // built for. Caveats: (a) find_task_by_pid
                                // may return a RECYCLED task after the
                                // original exited mid-episode (exec jumps
                                // wildly, dx saturates to 0) — treat wild
                                // drops as re-park-class noise, follow-up
                                // rows self-correct; (b) RT policies keep
                                // no CFS exec accounting (SchedRtEntity has
                                // no runtime field), so exec_ns reads 0
                                // forever — no adjudication possible.
                                let rt_policy = matches!(
                                    tk.policy(),
                                    crate::process::task::SchedPolicy::Fifo
                                        | crate::process::task::SchedPolicy::Rr
                                        | crate::process::task::SchedPolicy::Deadline
                                );
                                let exec_now = tk
                                    .sched_entity()
                                    .sum_exec_runtime
                                    .load(core::sync::atomic::Ordering::Relaxed);
                                let exec_prev = t
                                    .wrl_last_exec_ns
                                    .swap(exec_now, core::sync::atomic::Ordering::Relaxed);
                                let exec_dx = exec_now.saturating_sub(exec_prev);
                                let shape = if rt_policy {
                                    "rt-n/a"
                                } else if exec_prev != 0 && exec_dx == 0 {
                                    "never-ran"
                                } else {
                                    "re-park"
                                };
                                let _ = writeln!(
                                    out,
                                    "binder-stall: WAKE-RUN-LEAK tid {} ({}) kicked {}x age {}s state={:#x} cfs_linked={} on_cpu={} pick_state={} cpu={} pol={} on_rq={} dl_thr={} wok={} wno={} exec_ns={} exec_dx_ns={} shape={}",
                                    t.tid,
                                    comm,
                                    kicks + 1,
                                    age / hz,
                                    tk.state().bits(),
                                    linked,
                                    tk.on_cpu(),
                                    tk.pick_state(),
                                    tk.ti_cpu(),
                                    pol,
                                    onrq,
                                    if dlthr { 1 } else { 0 },
                                    t.kick_wake_ok.load(core::sync::atomic::Ordering::Relaxed),
                                    t.kick_wake_refused.load(core::sync::atomic::Ordering::Relaxed),
                                    exec_now,
                                    exec_dx,
                                    shape,
                                );
                                let _ = writeln!(out, "binder-stall:{}", slots);
                                // Watch this victim from now on — the ring
                                // retains its events between censuses.
                                crate::sched::hotpath::ww_watch_add(task_raw);
                                // Wakewatch tail: which scheduler stage last
                                // touched the victim and when (W wake-ok,
                                // L link, P unlink-pick, S unlink-sched,
                                // D unlink-deq, K pick-publish, A affirm,
                                // R refused; @jiffies-ago).
                                crate::sched::hotpath::ww_append_dump(&mut out, task_raw, now);
                                // WW-PROBE: re-issue the wake RIGHT HERE and
                                // read back linkage+state in the same breath.
                                // If wake_up_process returns true while
                                // linked stays 0, the enqueue lied; if it
                                // links and the NEXT census still shows
                                // unlinked+never-run, something unlinked a
                                // parked task without running it.
                                let pre_linked = crate::sched::hotpath::grq_diag_cfs_linked(task_raw);
                                let probe_ret = crate::sched::hotpath::wake_up_process(task_raw);
                                let post_linked = crate::sched::hotpath::grq_diag_cfs_linked(task_raw);
                                let post_state = tk.state();
                                let _ = writeln!(
                                    out,
                                    "binder-stall:  WW-PROBE ret={} linked {}->{} state={:#x}",
                                    probe_ret,
                                    match pre_linked {
                                        Some(true) => "1",
                                        Some(false) => "0",
                                        None => "?",
                                    },
                                    match post_linked {
                                        Some(true) => "1",
                                        Some(false) => "0",
                                        None => "?",
                                    },
                                    post_state.bits(),
                                );
                            }
                        }
                        rekick.push(t.clone());
                    }
                    if age < STALL_SECS * hz {
                        continue;
                    }
                    if t.stall_reported.swap(true, core::sync::atomic::Ordering::AcqRel) {
                        continue;
                    }
                    let secs = (age / hz).max(1);
                    let th = t.st(world);
                    let pending = th.txn_stack.len();
                    let owed_by = th.txn_stack.last().map(|txn| txn.to_proc.clone());
                    let top_txn = th.txn_stack.last().cloned();
                    use core::fmt::Write;
                    match (pending, &owed_by) {
                        (0, _) => {
                            let _ = writeln!(
                                out,
                                "binder-stall: pid {} tid {} waiting {}s for work (no pending txn)",
                                proc.pid, t.tid, secs
                            );
                        }
                        (n, Some(owner)) => {
                            // sc74 discriminator: the top txn is either one
                            // this thread SENT (from_tid == self — classic
                            // client waiting for its reply) or one it is
                            // SERVICING (to_tid == self — a server parked in
                            // read owing a reply, the death-loop terminal
                            // shape; to_proc == own pid is NOT sufficient to
                            // tell them apart with same-process IPC).
                            let shape = match &top_txn {
                                Some(txn) if txn.to_tid(world) == Some(t.tid) => "SERVER-OWING",
                                Some(txn) if txn.from_tid == Some(t.tid) => "CLIENT-WAIT",
                                _ => "CLIENT-WAIT?",
                            };
                            let _ = writeln!(
                                out,
                                "binder-stall: pid {} tid {} waiting {}s for reply from pid {} ({} txn(s) stacked) {}",
                                proc.pid, t.tid, secs, owner.pid, n, shape
                            );
                            if let Some(txn) = top_txn {
                                let _ = writeln!(
                                    out,
                                    "binder-stall:   top txn{} code {} oneway={} from pid {} tid {} to_tid {}",
                                    txn.id,
                                    txn.code,
                                    txn.flags & TF_ONE_WAY != 0,
                                    txn.from_proc.as_ref().map(|p| p.pid).unwrap_or(0),
                                    txn.from_tid.unwrap_or(0),
                                    txn.to_tid(world).unwrap_or(0)
                                );
                            }
                            // SERVER-OWING: does a nested txn from this
                            // thread exist anywhere in the world?
                            if shape == "SERVER-OWING" {
                                let from = outstanding_from_tid(world, proc.pid, t.tid);
                                if from.is_empty() {
                                    let _ = writeln!(
                                        out,
                                        "binder-stall:   NO outstanding txn from tid {} — nested call vanished or never sent",
                                        t.tid
                                    );
                                    // sc77: the impossible shape gets a user-stack
                                    // snapshot — names the OH path that parked.
                                    // sc89: deferred past the guard (page-walk).
                                    late_parkdumps.push(t.clone());
                                } else {
                                    let _ = writeln!(
                                        out,
                                        "binder-stall:   outstanding from tid {}:{}",
                                        t.tid,
                                        from
                                    );
                                }
                            }
                            let _ = writeln!(out, "binder-stall:{}", thread_census_of(t));
                            let _ = writeln!(out, "binder-stall:{}", hist_line_of(t.tid, &hist_snapshot(world, t)));
                            if dump_owners.len() < 2 && !dump_owners.iter().any(|o| Arc::ptr_eq(o, owner)) {
                                dump_owners.push(owner.clone());
                            }
                        }
                        (n, None) => {
                            let _ = writeln!(
                                out,
                                "binder-stall: pid {} tid {} waiting {}s for reply ({} txn(s) stacked)",
                                proc.pid, t.tid, secs, n
                            );
                        }
                    }
                }
            }
            // Full dump of up to two owing procs: proc todo depth + every
            // thread's looper/wait/stack/todo state. sc89: lines formatted
            // and hist rings snapshotted under the guard; census lines,
            // holder forensics, futex graphs and all printing are deferred.
            for owner in &dump_owners {
                if let Some(op) = world.procs.iter().find(|p| Arc::ptr_eq(p, owner)) {
                    let opm = pm(world, op);
                    // Name the owing process via its main thread's comm.
                    let pname = crate::process::find_task_by_pid(op.pid)
                        .map(|mt| {
                            let c = mt.comm();
                            core::str::from_utf8(&c[..])
                                .unwrap_or("?")
                                .trim_end_matches('\0')
                        })
                        .unwrap_or("?");
                    let mut od = OwnerDump {
                        pid: op.pid,
                        name: alloc::string::String::from(pname),
                        todo_depth: opm.im(world).todo.len(),
                        lines: Vec::new(),
                        hists: Vec::new(),
                        futex_line: alloc::string::String::new(),
                        threads: Vec::new(),
                    };
                    for t in &opm.im(world).threads {
                        let mut line = alloc::string::String::new();
                        // (unsafe-slim r3) token-checked accessors.
                        stall_thread_line(world, op, t, &mut line);
                        let has_stack = !t.st(world).txn_stack.is_empty();
                        od.hists.push(if has_stack {
                            Some(hist_snapshot(world, t))
                        } else {
                            None
                        });
                        od.threads.push(t.clone());
                        od.lines.push((line, has_stack));
                    }
                    late_futex_graph.push(op.pid);
                    owner_dumps.push(od);
                }
            }
        }
    }
    // Heals, outside WORLD (wake_proc_one takes the world lock itself).
    for t in rekick {
        wake_thread(&t);
    }
    // leg-32B: retired-slot handoff txns delivered to child threads.
    for t in retire_wake_ths.drain(..) {
        wake_thread(&t);
    }
    for p in retodo {
        wake_proc_one(&p, 0);
    }
    for p in retire_wakes.drain(..) {
        wake_proc_one(&p, 0);
    }
    // ---- sc89: slow forensics + output, all with irqs on, WORLD free ----
    use core::fmt::Write;
    for od in &owner_dumps {
        let _ = writeln!(
            out,
            "binder-stall: owing pid {} ('{}') proc todo depth {} threads {}",
            od.pid,
            od.name,
            od.todo_depth,
            od.lines.len()
        );
        for (((line, has_stack), hist), thr) in od
            .lines
            .iter()
            .zip(od.hists.iter())
            .zip(od.threads.iter())
        {
            let _ = writeln!(out, "binder-stall:{}", line);
            if *has_stack {
                let _ = writeln!(out, "binder-stall:{}", thread_census_of(thr));
                if let Some(ring) = hist {
                    let _ = writeln!(out, "binder-stall:{}", hist_line_of(thr.tid, ring));
                }
                { holder_forensics(thr, &mut out) };
            }
        }
    }
    // sc75: the owing proc's full futex-waiter graph — the
    // intra-process contention set (each entry: waiting tid,
    // uaddr, woken). The wedge terminal so far is handlers
    // blocked on per-thread-stack condvar/mutex words.
    for pid in late_futex_graph {
        let fw = crate::sync::futex::waiters_of_tgid(pid);
        if !fw.is_empty() {
            let mut s = alloc::string::String::new();
            for (wpid, uaddr, woken) in fw {
                let _ = write!(s, " [{}:{:#x} w={}]", wpid, uaddr, woken);
            }
            let _ = writeln!(out, "binder-stall: futex waiters of pid {}:", pid);
            let _ = writeln!(out, "binder-stall:{}", s);
        }
    }
    // Impossible-shape user-stack snapshots: the page-table walks.
    for t in &late_parkdumps {
        // SAFETY: no binder state touched (task lookup + mm walk).
        user_park_stack_dump(t);
    }
    // One flush, interrupts on. Chunked: split on the last newline below
    // ~950 bytes so no single console write stalls the console lock.
    if !out.is_empty() {
        let bytes = out.as_bytes();
        let mut start = 0usize;
        while start < bytes.len() {
            let mut end = (start + 950).min(bytes.len());
            if end < bytes.len() {
                // extend/cut to a line boundary
                match bytes[start..end].iter().rposition(|&b| b == b'\n') {
                    Some(rel) if rel > 200 => end = start + rel + 1,
                    _ => {
                        match bytes[start..].iter().position(|&b| b == b'\n') {
                            Some(rel) if rel < 1600 => end = start + rel + 1,
                            _ => {}
                        }
                    }
                }
            }
            let chunk = core::str::from_utf8(&bytes[start..end]).unwrap_or("");
            crate::pr_debug!("binder-stall:{}", chunk);
            start = end;
        }
    }
}


/// Register an appspawn child pid (sc140: written by the clone census
/// at each appspawn fork, first 128 per boot). MOVED from
/// kernel/core/src/ipc/binder.rs (cold-four lane) — its only caller is
/// syscall's clone path; the APP_CHILD_PIDS static stays
/// framework-side with the txn-path reader.
pub fn register_app_child_pid(pid: u32) {
    use crate::binder::APP_CHILD_PIDS;
    for slot in APP_CHILD_PIDS.iter() {
        let s = slot.load(core::sync::atomic::Ordering::Relaxed);
        if s == pid {
            return;
        }
        if s == 0 {
            let _ = slot.compare_exchange(
                0,
                pid,
                core::sync::atomic::Ordering::Relaxed,
                core::sync::atomic::Ordering::Relaxed,
            );
            return;
        }
    }
}
