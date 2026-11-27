//! The FUTEX COLD DIAGNOSTIC FAMILY (moved from
//! kernel/core/src/sync/futex.rs at the sync crate flip — sole copies here;
//! zero behavior change): the hunt-era forensic instruments accumulated
//! across the OH stall hunts — the sqwalk single-word walk family, the
//! ffrt mutex/lock scans (chain walk, region scan, holder scan, heap
//! map), the oh67/oh68 holder decoders, the rwfrz freeze-scan family
//! (freeze_scan, waiter clusters, neighbor dump, the exit ring and
//! kill-dump wedges), the ctd CoTask decoder, malloc_holder_scan,
//! musl anchor classification, the fuxw wake-ring summaries and the
//! boot-time fuxwatch cmdline parsers.
//!
//! What stayed framework-side (kernel/core/src/sync/futex.rs — the worklist's
//! over-keeps, each recorded in the lane report):
//! - the futex MECHANISM: FutexKey + resolve_futex_phys, the waiter
//!   pool + hash buckets, wait/wake/requeue/wake_op, do_futex,
//!   sys_futex_handler (their inline watch-latch sections call back
//!   through sync_diag_hooks rows — arm-gated, the S1 pattern);
//! - futex_stuck_census (1,803 lines): timer.rs's tick heartbeat calls
//!   it by name — the stay rule; its own calls into THIS file ride
//!   sync_diag_hooks rows inside its armed, rate-capped body;
//! - the fuxwatch watchpoint machinery the x86 trap fault/single-step
//!   paths call by name (log_write, take_step, page_watched,
//!   page_matches, mm_armed, read_word_at, addr, 2_addr, arm_round,
//!   retire_*) plus its statics;
//! - the statics the staying mechanism reads/writes (FXDV_*, FXRS_TGID,
//!   FUXW_RINGS, FUXPARK_*, FUXWATCH_*, RWFRZ_WATCH*, RWFRZ_UADDR,
//!   RWFRZ_OPLOG, OH67 keys) — published `pub`, this file reaches them
//!   as direct downward reads/writes (atomics; safe cross-crate);
//! - waiter_of / apppark_rets / probe_file_path / rwfz_stack_frames /
//!   rd_word / oh68_fs_base / oh68_ctx_of: consumed by staying files
//!   (census, sys_futex_handler, fuxwatch_log_write) — published pub,
//!   called downward from here.
//!
//! Reaching THIS file:
//! - sibling service crates (ipc's binder_diag, process) call
//!   the moved fns DIRECTLY at their historical rux::sync::hotpath::X
//!   paths (the union module in this crate's lib.rs; the allowed
//!   downward service edge — the binder_diag precedent);
//! - framework cold sites (exit, pid_hash, signal kill dumps, taskdump,
//!   schb, debug switches, binder devmgr latch, the do_futex/census
//!   latch sections) ride the SyncDiagHooks rows
//!   (kernel/core/src/api/sync.rs) — cold rows only.

// The published futex surface this file consumes (see the header): the
// watch statics, the waiter pool + hash heads, and the walker helpers
// that stayed framework-side.
#[allow(unused_imports)]
use crate::futex::{FUXPARK_J, FUXPARK_TID};
use rux::sync::{
    hotpath::FUXPARK_TGID, hotpath::FUXPARK_WORD, hotpath::FUXWATCH1_TGID,
    hotpath::FUXWATCH2_TGID, hotpath::FUXWATCH_ADDR, hotpath::FUXWATCH2_ADDR, hotpath::FUXWATCH_DELAY_S,
    hotpath::FUXWATCH_MM_DONE, hotpath::FUXWATCH_OWNER_TASK, hotpath::FUXW_RING_SZ, hotpath::FUXW_RINGS,
    hotpath::FXDV_PID, hotpath::FXRS_TGID, hotpath::HASH_HEADS, hotpath::HASH_SIZE, hotpath::HEAPMAP_ARM, hotpath::OH68_KEY,
    hotpath::OH68_OWNER_CTX, hotpath::RWFRZ_EXIT_WATCH_TGID, hotpath::RWFRZ_OPLOG, hotpath::RWFRZ_UADDR,
    hotpath::RWFRZ_WATCH,
    hotpath::RWFRZ_WATCH_BUDGET, hotpath::WAITER_POOL, types::Waiter, hotpath::apppark_rets,
    hotpath::fuxwatch_log_write, hotpath::fuxwatch_retire_page,
    hotpath::oh68_ctx_of, hotpath::oh68_fs_base, hotpath::rd_word, hotpath::rwfz_emit_prio, hotpath::rwfz_stack_frames,
};
#[allow(unused_imports)]
use rux::sync::types::Spinlock;
#[allow(unused_imports)]
use crate::process::types::Task;
#[allow(unused_imports)]
use core::sync::atomic::AtomicU32;

/// The exit-path forensics tgid (0 = off).
pub fn rwfz_exit_watch_tgid() -> u32 {
    RWFRZ_EXIT_WATCH_TGID.load(core::sync::atomic::Ordering::Relaxed)
}

/// Latch the hdf_devmgr pid for the FXDV tracer (idempotent; first caller
/// wins — binder.rs calls this when the SA 5100 registration is seen).
pub fn fxdv_latch_devmgr(pid: u32) {
    let prev = FXDV_PID.swap(pid, core::sync::atomic::Ordering::Relaxed);
    if prev != pid {
        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
        crate::pr_warn!("FXDV latch devmgr pid={}", pid);
    }
}

// sc209 (leg 14): render_service futex lifecycle tracer. sc208 closed the
// SA-10 question to the publisher side: render_service main runs through
// composer/HGM init, logs "HgmTaskHandleThread init" (hgm_context.cpp:60),
// then parks in a futex forever — the very next source statement is
// InitHgmTaskHandleThread's PostSyncTask (an FFRT queue->wait(handle)),
// and AddSystemAbility(10) never runs (no "insert SA:10" all boot). The
// census sleepsnap shows its FFRT workers (OS_FFRT_5_0/_1) alive but
// asleep. This tracer latches the render_service tgid at exec and prints
// EVERY futex WAIT/WAKE/REQUEUE from that process (entry + return): a
// WAKE issued to the parked main's uaddr that it never consumes = kernel
// wake loss; silence = the FFRT worker never ran the task (userspace
// chain — the next hop is the worker's own wait lines).

/// Latch the render_service tgid for the FXRS tracer (idempotent; called
/// from the exec path when /system/bin/render_service loads).
pub fn fxrs_latch(tgid: u32) {
    let prev = FXRS_TGID.swap(tgid, core::sync::atomic::Ordering::Relaxed);
    if prev != tgid {
        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
        crate::pr_warn!("FXRS latch render_service tgid={}", tgid);
    }
}

// sc185 RWFRZ-WATCH: words named by RWFRZ-PARK (writers parked on
// 0x80000000|N readers), watched for their post-park futex lifecycle from
// ANY process. A later WAKE on the word that the parked writer never
// consumes = kernel wake loss; silence forever = readers never released
// (userspace). See the watch block in do_futex and the registration in the
// RWFRZ-PARK hook.

/// Diagnostic (sc75 launcher IPC death loop): every live futex waiter that
/// belongs to `tgid` — the intra-process contention graph. Returns
/// (task_pid, uaddr, woken) triples; the binder stall dump prints them so a
/// handler blocked on a per-call condvar/mutex names the whole wait set and
/// each one's woken state. Entries whose task is no longer in the pid hash
/// are skipped (exit-race guard for the raw task deref).
pub fn waiters_of_tgid(tgid: u32) -> alloc::vec::Vec<(u32, usize, bool)> {
    let mut out = alloc::vec::Vec::new();
    for b in HASH_HEADS.iter() {
        let h = b.lock_irqsave();
        let mut idx = h.head;
        while let Some(i) = idx {
            // SAFETY: slot indices come from chain links under the bucket
            // lock; the tuple is copied out under the slot lock.
            let (entry, next) = {
                let slot = WAITER_POOL[i].lock_irqsave();
                match *slot {
                    Some(ref w) if !w.task.is_null() => {
                        // SAFETY: liveness verified via the pid hash before
                        // the tgid read; the derefs touch only pid fields.
                        let t = crate::sched::r#unsafe::task_shared(w.task).pid();
                        let live = core::ptr::eq(
                            // SAFETY: diagnostic pid-hash lookup.
                            crate::sched::hotpath::find_task_by_pid(t),
                            w.task,
                        );
                        let hit = live && (crate::sched::r#unsafe::task_shared(w.task).tgid() as u32 == tgid);
                        (Some((hit, t, w.key.uaddr, w.woken)), w.next)
                    }
                    Some(ref w2) => (None, w2.next),
                    None => (None, None),
                }
            };
            match entry {
                Some((true, pid, uaddr, woken)) => out.push((pid, uaddr, woken)),
                Some((false, ..)) => {}
                None => break,
            }
            idx = next;
        }
        drop(h);
    }
    out
}

/// sc57 (57th session): FFRT waiter-chain walk for an arbitrary parked
/// task — the export the binder TXN-HANG-BWR report calls (the futex
/// census has its own inline copy; the waiter-pool census cannot see
/// waiters parked after the pool filled). Layout rationale in the
/// census (d) block comment: scan the parked task's entry-sp window
/// for heap-class pointers, test each as a WaitUntilEntry.cv /
/// wn.wl spill with node offsets {0x60,0x70,0x88,0x98}, verify the
/// intrusive LinkedList ring by back-links, walk it to the head (a
/// member of the contested mutexPrivate / SharedMutexPrivate), and
/// window the head. Returns a printable summary, or None when the
/// park does not decode. debug-gated by the caller; walker-only reads
/// of the pinned waiter's frozen mm.
pub fn ffrt_chain_walk(
    task: *mut crate::process::types::Task,
    uaddr: usize,
) -> Option<(alloc::string::String, Option<u64>)> {
    use core::fmt::Write as _;
    if task.is_null() {
        return None;
    }
    // SAFETY: diagnostic reads of the parked task under the
    // for_each/try contract; the task is pinned by the caller's pid
    // lookup and parked in the futex this walk decodes.
    let tk = crate::sched::r#unsafe::task_shared(task);
    let top = tk.kernel_stack_top_checked()?;
    #[cfg(feature = "x86_64")]
    let usp = crate::sched::r#unsafe::pt_regs_at_top(top).rsp as u64;
    #[cfg(feature = "riscv64")]
    let usp = crate::sched::r#unsafe::pt_regs_at_top(top).sp as u64;
    #[cfg(feature = "aarch64")]
    let usp = crate::sched::r#unsafe::pt_regs_at_top(top).sp as u64;
    if usp == 0 {
        return None;
    }
    let aspace = tk.address_space()?;
    let root = aspace.root_ppn();
    let mgr = aspace.vma_try_read()?;
    let rd = |va: u64| -> Option<u64> {
        // SAFETY: read-only page-table walk of the parked task's mm.
        match rux::sync::r#unsafe::pt_walk(root, va) {
            Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
                let kv = crate::arch::mm::memory_layout::phys_to_virt(
                    crate::arch::mm::memory_layout::PhysAddr::new((ppn << 12) | (va & 0xfff)),
                );
                // SAFETY: linear-mapped RAM word.
                Some(rux::sync::r#unsafe::read_volatile_u64(kv.as_usize()))
            }
            _ => None,
        }
    };
    let heapish = |va: u64| -> bool {
        if va <= 0x10000 || (va >> 47) != 0 {
            return false;
        }
        match mgr.find(crate::mm::types::VirtAddr::new(va as usize)) {
            Some(vma) => {
                !vma.flags().is_executable()
                    && !vma.flags().contains(crate::mm::types::VmaFlags::GROWSDOWN)
            }
            None => false,
        }
    };
    let _ = uaddr; // the park word itself is a stack word (musl node);
                   // the scan starts from the frame spills instead
    for wi in 0..96u64 {
        let Some(w) = rd(usp + 8 * wi) else {
            continue;
        };
        if !heapish(w) {
            continue;
        }
        for &xoff in &[0x60u64, 0x70, 0x88, 0x98] {
            let node = w.wrapping_sub(xoff);
            if !heapish(node) {
                continue;
            }
            let (Some(prev), Some(next)) = (rd(node), rd(node + 8)) else {
                continue;
            };
            if prev == 0
                || next == 0
                || prev == node
                || next == node
                || !heapish(next)
                || !heapish(prev)
            {
                continue;
            }
            let ok_next = rd(next) == Some(node);
            let ok_prev = rd(prev + 8) == Some(node);
            if !ok_next && !ok_prev {
                continue;
            }
            let mut out = alloc::string::String::new();
            let _ = write!(out, "node={:#x} nwin:", node);
            for hw in 0..6u64 {
                if let Some(x) = rd(node.wrapping_sub(0x18) + 8 * hw) {
                    let _ = write!(out, " {:#x}", x);
                }
            }
            // mutex-shape test: the ring member that IS the lock's
            // list head has (head-0x8) low32 == 2 (sync_detail::WAIT)
            // and (head-0x10) a file-backed non-exec pointer (the
            // mutexPrivate vtable in some .so's data).
            let filebacked = |va: u64| -> bool {
                match mgr.find(crate::mm::types::VirtAddr::new(va as usize)) {
                    Some(vma) => {
                        !vma.flags().is_executable()
                            && matches!(
                                vma.vma_type(),
                                crate::mm::types::VmaType::FileBacked
                            )
                    }
                    None => false,
                }
            };
            // 61st (LOCKHEAD vtable-vs-data disambiguation): a REAL C++
            // object's +0 slot points at a VTABLE whose first slot is a
            // FUNCTION pointer into an exec mapping; a VersionCtx-class
            // false positive (+0 = a file-backed DATA pointer) fails the
            // deref test. ohw8/9's auto-arm fired on the data-pointer
            // class and the walk then "decoded" version-chain churn as
            // mutex traffic; require the deref hit here.
            let exec_ptr = |va: u64| -> bool {
                match mgr.find(crate::mm::types::VirtAddr::new(va as usize)) {
                    Some(vma) => vma.flags().is_executable(),
                    None => false,
                }
            };
            let vtable_like = |v: u64| -> bool {
                filebacked(v) && rd(v).map_or(false, exec_ptr)
            };
            let lock_shape = |hop: u64| -> bool {
                match (rd(hop.wrapping_sub(0x10)), rd(hop.wrapping_sub(0x8))) {
                    (Some(v), Some(l)) => {
                        // l low32: 1 = LOCK (held, no waiters flagged yet),
                        // 2 = WAIT (held with waiters); 0 = free.
                        vtable_like(v)
                            && ((l & 0xffff_ffff) == 1 || (l & 0xffff_ffff) == 2)
                    }
                    _ => false,
                }
            };
            let _ = write!(out, " hops:");
            let mut cur = node;
            for h in 0..16 {
                let Some(nxt) = rd(cur + 8) else { break };
                if nxt == node {
                    break;
                }
                if nxt == 0 || (nxt >> 47) != 0 || !heapish(nxt) {
                    break;
                }
                let _ = write!(out, " {:#x}{}", nxt, if rd(nxt) == Some(cur) { "*" } else { "?" });
                let is_lock = lock_shape(nxt);
                // window every hop (8 words at hop-0x28)
                if rd(nxt.wrapping_sub(0x28))
                    .map_or(false, |x| x == 0 || (x >> 47) == 0)
                {
                    let mut win = alloc::string::String::new();
                    let mut nz = 0;
                    for hw in 0..8u64 {
                        if let Some(x) = rd(nxt.wrapping_sub(0x28) + 8 * hw) {
                            if x != 0 {
                                nz += 1;
                            }
                            let _ = write!(win, " {:#x}", x);
                        }
                    }
                    if nz > 0 {
                        let _ = write!(out, " w[{}:{}]", nxt, win);
                    }
                }
                if is_lock {
                    // sc57b: auto-arm a watch on the DETECTED lock's `l`
                    // word (hop-0x8: the mutexPrivate state) once per
                    // boot — ONLY for the registered samgr pid's walks:
                    // ohc71 armed a random hot mutex from a foreign
                    // process's ring and trapped the boot into a
                    // fault+step crawl. 60th: prefer SLOT 1 (the manual
                    // fuxwatch= param is absent on these boots and the
                    // same-VA word across processes is a different
                    // object every boot); fall back to slot 2. The old
                    // census auto-arm (first stuck OS_FFRT park word =
                    // a musl cv waiter node on the waiter's own stack)
                    // cannot identify the owner and was stealing the
                    // slot before this walk could arm the lock word —
                    // it no longer fires.
                    let samgr_pid =
                        crate::ipc::hotpath::binder_sareg_samgr_pid();
                    if samgr_pid != 0 && tk.tgid() as u32 == samgr_pid {
                        let wl = nxt.wrapping_sub(0x8);
                        let w1 = FUXWATCH_ADDR.load(core::sync::atomic::Ordering::Relaxed);
                        if (w1 & !0x3) != (wl & !0x3) {
                            if FUXWATCH_ADDR
                                .compare_exchange(
                                    0,
                                    wl,
                                    core::sync::atomic::Ordering::Relaxed,
                                    core::sync::atomic::Ordering::Relaxed,
                                )
                                .is_err()
                            {
                                FUXWATCH2_ADDR
                                    .compare_exchange(
                                        0,
                                        wl,
                                        core::sync::atomic::Ordering::Relaxed,
                                        core::sync::atomic::Ordering::Relaxed,
                                    )
                                    .ok();
                            }
                            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                            crate::pr_warn!(
                                "FUXWATCH-LOCKHEAD l={:#x} head={:#x} tid={} pid={}",
                                wl,
                                nxt,
                                tk.pid(),
                                samgr_pid
                            );
                        }
                    }
                    return Some((out, Some(nxt)));
                }
                cur = nxt;
            }
            // no closure within budget: report the hops anyway plus
            // the prev direction
            let _ = write!(out, " prev:");
            let mut cur = node;
            for _ in 0..3 {
                let Some(prv) = rd(cur) else { break };
                if prv == 0 || prv == node || (prv >> 47) != 0 || !heapish(prv) {
                    break;
                }
                let _ = write!(out, " {:#x}", prv);
                cur = prv;
            }
            return Some((out, None));
        }
    }
    None
}

/// 61st (ohw12): OWNER-directed mutex scan — the ffrt_chain_walk starts
/// from a WAITER's ring and finds the contested lock; this scan starts
/// from the OWNER's own frame spills and names the locks the owner
/// touches: the suspended critical-section task still holds `this` of
/// the NEXT mutex it tried (mutexPrivate::wait's frame spills rdi=M2)
/// or of the mutex it holds. Scans the parked task's entry-sp window
/// (256 words) for heap pointers that shape-test as mutexPrivate
/// objects (vtable-deref + l∈{1,2} at +0x8), walks each hit's waiter
/// ring, and prints the mutex VA + l + ring so the M2→M3 chain can be
/// assembled across threads. Walker-only reads of the pinned task's
/// frozen mm; debug-gated by the caller. Returns None when nothing in
/// the window shape-tests (plain cond/mutex parks).
/// 77th: the most recent CONTENDED (l==2) mutexPrivate lock word seen
/// by any ffrt_mutex_scan — the TXN-HANG print site auto-arms fuxwatch
/// slot 2 on it (the samgr load-wedge's live word, which shifts per
/// boot: 0x7ff0004b5ec8 on ohw108-110 vs 0x7ff000499a48 on ohw112).
pub static FRMTX_LIVE_WORD: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

pub fn ffrt_mutex_scan(task: *mut crate::process::types::Task) -> Option<alloc::string::String> {
    use core::fmt::Write as _;
    if task.is_null() {
        return None;
    }
    // SAFETY: diagnostic reads of the parked task under the
    // for_each/try contract; the task is pinned by the caller's pid
    // lookup and parked in the futex this scan decodes.
    let tk = crate::sched::r#unsafe::task_shared(task);
    let top = tk.kernel_stack_top_checked()?;
    #[cfg(feature = "x86_64")]
    let usp = crate::sched::r#unsafe::pt_regs_at_top(top).rsp as u64;
    #[cfg(feature = "riscv64")]
    let usp = crate::sched::r#unsafe::pt_regs_at_top(top).sp as u64;
    #[cfg(feature = "aarch64")]
    let usp = crate::sched::r#unsafe::pt_regs_at_top(top).sp as u64;
    if usp == 0 {
        return None;
    }
    let aspace = tk.address_space()?;
    let root = aspace.root_ppn();
    let mgr = aspace.vma_try_read()?;
    let rd = |va: u64| -> Option<u64> {
        // SAFETY: read-only page-table walk of the parked task's mm.
        match rux::sync::r#unsafe::pt_walk(root, va) {
            Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
                let kv = crate::arch::mm::memory_layout::phys_to_virt(
                    crate::arch::mm::memory_layout::PhysAddr::new((ppn << 12) | (va & 0xfff)),
                );
                // SAFETY: linear-mapped RAM word.
                Some(rux::sync::r#unsafe::read_volatile_u64(kv.as_usize()))
            }
            _ => None,
        }
    };
    let heapish = |va: u64| -> bool {
        if va <= 0x10000 || (va >> 47) != 0 {
            return false;
        }
        match mgr.find(crate::mm::types::VirtAddr::new(va as usize)) {
            Some(vma) => {
                !vma.flags().is_executable()
                    && !vma.flags().contains(crate::mm::types::VmaFlags::GROWSDOWN)
            }
            None => false,
        }
    };
    let exec_ptr = |va: u64| -> bool {
        match mgr.find(crate::mm::types::VirtAddr::new(va as usize)) {
            Some(vma) => vma.flags().is_executable(),
            None => false,
        }
    };
    // mutexPrivate shape at base B (DWARF-verified): +0 = vtable ptr
    // (file-backed, derefs to exec), +0x8 = l word in {1,2}, +0x10/+0x18
    // = waiter LinkedList {prev,next} — self-linked when idle, otherwise
    // a closed ring whose first node's prev points back at B+0x10.
    // 61st (ohw14): ExecuteCtx ALSO has a vtable (virtual dtor) and its
    // +0x8 QoS (value 1 or 2) mimics the l word — the first cut matched
    // every parked thread's own ctx (m1=...l2 with an empty ring). The
    // ctx's +0x10 is the task ptr and +0x18 the 8-byte-hole garbage —
    // both fail the ring-head test below.
    let mutex_at = |b: u64| -> bool {
        match (rd(b), rd(b + 0x8), rd(b + 0x10), rd(b + 0x18)) {
            (Some(v), Some(l), Some(p), Some(n)) => {
                ((l & 0xffff_ffff) == 1 || (l & 0xffff_ffff) == 2)
                    && heapish(v)
                    && rd(v).map_or(false, exec_ptr)
                    && ((p == b + 0x10 && n == b + 0x10)
                        || (heapish(n) && rd(n) == Some(b + 0x10)))
            }
            _ => false,
        }
    };
    let mut out = alloc::string::String::new();
    let mut nhit = 0usize;
    let mut seen_locks: [u64; 4] = [0; 4];
    for wi in 0..256u64 {
        if nhit >= 4 {
            break;
        }
        let Some(w) = rd(usp + 8 * wi) else {
            continue;
        };
        if !heapish(w) {
            continue;
        }
        // The frame may spill &m (mutexPrivate*) or &m->l — test both.
        for &b in &[w, w.wrapping_sub(0x8)] {
            if seen_locks.contains(&b) || !heapish(b) {
                continue;
            }
            if !mutex_at(b) {
                continue;
            }
            seen_locks[nhit % 4] = b;
            nhit += 1;
            let l = rd(b + 0x8).unwrap_or(0) & 0xffff_ffff;
            // 77th: publish the first CONTENDED (l==2) mutex's lock word
            // for the TXN-HANG auto-arm (FUXWATCH2) — the wedge's live
            // word shifts per boot (0x7ff0004b5ec8 on ohw108-110 vs
            // 0x7ff000499a48 on ohw112), so a fixed cmdline address
            // misses; the chain scan sees the real word every burst.
            if l == 2 {
                FRMTX_LIVE_WORD.store(b + 0x8, core::sync::atomic::Ordering::Relaxed);
            }
            let _ = write!(out, " m{nhit}={:#x}l{l}(", b);
            // Waiter ring: head node at b+0x10 (next at +0x0, prev +0x8).
            let head = b + 0x10;
            let mut cur = head;
            let mut hops = 0;
            for _ in 0..12 {
                let Some(nxt) = rd(cur) else { break };
                if nxt == head {
                    break;
                }
                if nxt == 0 || !heapish(nxt) || hops >= 8 {
                    break;
                }
                let _ = write!(out, "{}{:#x}", if hops == 0 { "" } else { "," }, nxt);
                cur = nxt;
                hops += 1;
            }
            let _ = write!(out, ") ");
            break;
        }
    }
    if nhit == 0 {
        return None;
    }
    Some(out)
}

// sc57 (57th): kernel-side write watchpoint (the coordinator's TCG
// hardware-watchpoint rig, in-kernel and KVM-fast). fuxwatch=0xADDR
// names a userspace word whose address is deterministic per image
// (the ffrt mutexPrivate `l` word of the wedge's contested lock has
// been 0x7ff0004862b8+8 in every boot); the censusd kthread clears
// the PTE W bit on every mm that maps it; the write-fault path logs
// the write (tid, rip), restores W, single-steps the writer via TF
// (the #DB hook re-clears W), so EVERY write to the word is logged
// with zero gap. The last write before the freeze names the owner
// that never unlocked. All zero-cost without the boot param.

/// Parse fuxwatch=0xADDR[,DELAYSEC] (and sc58: fuxwatch2=0xADDR) once at
/// boot (called from the debug init path). Empty/absent leaves the watch
/// off. fuxwatch2 pre-arms the second slot (the census/chain-walk
/// auto-arm only fills it when free).
pub fn fuxwatch_parse() {
    // rwlock-livelock: rwfrzwatch=0xADDR pre-arms the RWFRZ-WATCH futex
    // op tracer at boot (the census latches only minutes after a freeze
    // forms — the holder's final unlock lands before that and was
    // invisible). For a deterministic image (the ub2404 spawn-queue
    // mutex address is stable across boots) this records the word's
    // COMPLETE WAIT/WAKE lifecycle from t=0.
    if let Some(v) = crate::cmdline::get_param("rwfrzwatch") {
        if let Some(a) = v.trim_start_matches("0x").split(',').next() {
            if let Ok(addr) = u64::from_str_radix(a, 16) {
                if addr > 0x1000 {
                    RWFRZ_WATCH[0].store(addr as usize, core::sync::atomic::Ordering::Relaxed);
                    // HOTPATH-PRINT: switch/cmdline-gated (off by default).
                    crate::pr_warn!("rwfrzwatch armed {:#x}", addr);
                }
            }
        }
    }
    if let Some(v) = crate::cmdline::get_param("rwfrzbudget") {
        if let Ok(b) = v.parse::<usize>() {
            if b > 0 && b <= 200000 {
                RWFRZ_WATCH_BUDGET.store(b, core::sync::atomic::Ordering::Relaxed);
                // HOTPATH-PRINT: switch/cmdline-gated (off by default).
                crate::pr_warn!("rwfrzbudget {}", b);
            }
        }
    }
    let Some(v) = crate::cmdline::get_param("fuxwatch") else {
        // fuxwatch2 is independent of slot 1
        fuxwatch2_parse();
        return;
    };
    let mut it = v.split(',');
    let addr = it.next().and_then(|s| {
        let t = s.trim_start_matches("0x");
        u64::from_str_radix(t, 16).ok()
    });
    let delay = it.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    if let Some(a) = addr {
        if a > 0x1000 {
            FUXWATCH_ADDR.store(a, core::sync::atomic::Ordering::Relaxed);
            FUXWATCH_DELAY_S.store(delay, core::sync::atomic::Ordering::Relaxed);
            // HOTPATH-PRINT: switch/cmdline-gated (off by default).
            crate::pr_warn!("fuxwatch armed target={:#x} delay={}s", a, delay);
            // sc58: the arm rounds run in the censusd kthread, which was
            // only spawned lazily on the first DEFERRED print — ohc73/74
            // never queued one before the wedge window, so the watch
            // words never armed (0 FW lines with FUXWATCH2-AUTO armed).
            // Spawn censusd now (parse runs in early-boot task context).
            crate::debug::hotpath::ensure_defer_thread();
        }
    }
    fuxwatch2_parse();
}

pub fn fuxwatch2_parse() {
    let Some(v) = crate::cmdline::get_param("fuxwatch2") else {
        return;
    };
    let addr = v.split(',').next().and_then(|s| {
        let t = s.trim_start_matches("0x");
        u64::from_str_radix(t, 16).ok()
    });
    if let Some(a) = addr {
        if a > 0x1000 {
            FUXWATCH2_ADDR.store(a, core::sync::atomic::Ordering::Relaxed);
            // HOTPATH-PRINT: switch/cmdline-gated (off by default).
            crate::pr_warn!("fuxwatch2 armed target={:#x}", a);
            // sc58: censusd must exist for the arm rounds (see slot 1).
            crate::debug::hotpath::ensure_defer_thread();
        }
    }
}

/// 62nd: one-line wake history for a park word (the FUX-OWNER row's
/// wake-side verdict): how many futex ops in ring coverage ever
/// targeted the word, and the last few ops' tid/age/op/ret. The rings
/// hold ~6 s of ops, so ever=0 on an old park means no wake in the
/// recent window either — consistent with a userspace holder that
/// never signaled. A REPEATED ret=1 wake with the waiter still parked
/// is the wake-vs-repark class (the summary cannot tell — the park
/// state function below does).
pub fn fuxw_word_wake_summary(uaddr: u64, now: u32) -> alloc::string::String {
    use core::fmt::Write as _;
    let mut ever = 0u64;
    let mut last: alloc::vec::Vec<(u32, u32, u8, i32)> = alloc::vec::Vec::new();
    for ring in FUXW_RINGS.iter() {
        let g = ring.lock();
        let (idx, arr) = &*g;
        for k in 0..FUXW_RING_SZ {
            let i = (*idx + k) % FUXW_RING_SZ;
            let e = arr[i];
            if e.j == 0 || e.uaddr != uaddr {
                continue;
            }
            ever += 1;
            last.push((e.j as u32, e.tid, e.op, e.ret));
        }
    }
    if ever == 0 {
        return alloc::format!("ever=0");
    }
    last.sort_by_key(|e| core::cmp::Reverse(e.0));
    let mut s = alloc::string::String::new();
    let _ = write!(s, "ever={}", ever);
    for (j, tid, op, ret) in last.iter().take(4) {
        let _ = write!(
            s,
            " +{}ms tid{}(op{} ret{})",
            now.wrapping_sub(*j).saturating_mul(10),
            tid,
            op,
            ret
        );
    }
    s
}

/// 62nd: the owner's live park state from the kernel waiter pool —
/// (age_ms, woken_flag, other_waiters). age growing across bursts =
/// the waiter never woke (the wake never reached it, or the wake woke
/// a phantom); age resetting with ret=1 wakes = the wake-repark loop
/// (the lock it wants is re-taken before the waiter re-acquires).
/// other_waiters counts same-uaddr entries held by OTHER tasks — a
/// shared word whose wakes land on someone else.
pub fn fuxw_owner_park_state(tid: u32, uaddr: u64, now: u32) -> (u32, bool, usize) {
    let mut age = 0u32;
    let mut woken = false;
    let mut found = false;
    let mut others = 0usize;
    for slot in WAITER_POOL.iter() {
        let g = slot.lock_irqsave();
        match *g {
            Some(ref w) if !w.task.is_null() => {
                // SAFETY: liveness verified via the pid hash before the
                // tid read; the derefs touch only pid fields.
                let t = crate::sched::r#unsafe::task_shared(w.task).pid();
                if w.key.uaddr as u64 != uaddr {
                    continue;
                }
                if t == tid {
                    found = true;
                    age = now
                        .wrapping_sub(w.park_j as u32)
                        .saturating_mul(10);
                    woken = w.woken;
                } else {
                    others += 1;
                }
            }
            _ => {}
        }
    }
    if !found {
        (0, false, others)
    } else {
        (age, woken, others)
    }
}

/// 62nd: shape-test the owner's PARK WORD itself — if the word is an
/// l word (a musl pthread mutex or an ffrt::mutexPrivate in the
/// worker-pool regions the ±64MB heap map never swept), name the
/// object: [uaddr-0x10, uaddr-0x8=l, uaddr, +0x10, +0x18] plus the
/// vtable-deref-exec verdict at -0x10. Returns None when the window
/// is unreadable or the shape is a plain cv seq word.
pub fn fuxw_word_shape(task: *mut crate::process::types::Task, uaddr: u64) -> Option<alloc::string::String> {
    use core::fmt::Write as _;
    if task.is_null() {
        return None;
    }
    // SAFETY: diagnostic reads of the parked task under the caller's
    // pid-lookup pin; stack-top check only.
    let tk = crate::sched::r#unsafe::task_shared(task);
    if tk.kernel_stack_top_checked().is_none() {
        return None;
    }
    let aspace = tk.address_space()?;
    let root = aspace.root_ppn();
    let mgr = aspace.vma_try_read()?;
    let rd = |va: u64| -> Option<u64> {
        // SAFETY: read-only page-table walk of the parked task's mm.
        match rux::sync::r#unsafe::pt_walk(root, va) {
            Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
                let kv = crate::arch::mm::memory_layout::phys_to_virt(
                    crate::arch::mm::memory_layout::PhysAddr::new((ppn << 12) | (va & 0xfff)),
                );
                // SAFETY: linear-mapped RAM word.
                Some(rux::sync::r#unsafe::read_volatile_u64(kv.as_usize()))
            }
            _ => None,
        }
    };
    let filebacked = |va: u64| -> bool {
        match mgr.find(crate::mm::types::VirtAddr::new(va as usize)) {
            Some(vma) => {
                !vma.flags().is_executable()
                    && matches!(vma.vma_type(), crate::mm::types::VmaType::FileBacked)
            }
            None => false,
        }
    };
    let exec_ptr = |va: u64| -> bool {
        match mgr.find(crate::mm::types::VirtAddr::new(va as usize)) {
            Some(vma) => vma.flags().is_executable(),
            None => false,
        }
    };
    let vt = rd(uaddr.wrapping_sub(0x10))?;
    let l = rd(uaddr.wrapping_sub(0x8))?;
    let cur = rd(uaddr)?;
    let n10 = rd(uaddr + 0x10).unwrap_or(0);
    let n18 = rd(uaddr + 0x18).unwrap_or(0);
    let vtable_like = filebacked(vt) && rd(vt).map_or(false, exec_ptr);
    let lw = l & 0xffff_ffff;
    if !vtable_like {
        return None;
    }
    let mut s = alloc::string::String::new();
    let _ = write!(
        s,
        "mutex-shape l={} cur={} vtbl={:#x} ring=({:#x},{:#x})",
        lw, cur, vt, n10, n18
    );
    Some(s)
}

/// 62nd: deep user frame chain for a parked task — the rbp chain up to
/// 24 frames (raw rips for offline symbolization) plus a 24-word raw
/// scan above the entry rsp (the spill window the rbp chain skips).
/// Walker-only reads of the pinned task's mm (the ffrt_chain_walk
/// contract); None when the entry frame is unavailable.
pub fn deep_ufw(task: *mut crate::process::types::Task) -> Option<alloc::string::String> {
    use core::fmt::Write as _;
    if task.is_null() {
        return None;
    }
    // SAFETY: diagnostic reads of the parked task under the caller's
    // pid-lookup pin; the derefs touch only the stack-top check.
    let tk = crate::sched::r#unsafe::task_shared(task);
    let top = tk.kernel_stack_top_checked()?;
    #[cfg(feature = "x86_64")]
    let (urbp, ursp) = {
        // INV-SCE-9: the entry frame at the stack top.
        let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
        (regs.rbp as u64, regs.rsp as u64)
    };
    #[cfg(feature = "riscv64")]
    let (urbp, ursp) = {
        // INV-SCE-9: the entry frame at the stack top.
        let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
        (regs.s0 as u64, regs.sp as u64)
    };
    #[cfg(feature = "aarch64")]
    let (urbp, ursp) = {
        // INV-SCE-9: the entry frame at the stack top.
        let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
        (regs.regs[29] as u64, regs.sp as u64)
    };
    if ursp == 0 {
        return None;
    }
    let aspace = tk.address_space()?;
    let root = aspace.root_ppn();
    let rd = |va: u64| -> Option<u64> {
        // SAFETY: read-only page-table walk of the parked task's mm.
        match rux::sync::r#unsafe::pt_walk(root, va) {
            Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
                let kv = crate::arch::mm::memory_layout::phys_to_virt(
                    crate::arch::mm::memory_layout::PhysAddr::new((ppn << 12) | (va & 0xfff)),
                );
                // SAFETY: linear-mapped RAM word.
                Some(rux::sync::r#unsafe::read_volatile_u64(kv.as_usize()))
            }
            _ => None,
        }
    };
    let mut out = alloc::string::String::new();
    let mut rbp = urbp;
    let _ = write!(out, "rbp={:#x} rsp={:#x} chain:", rbp, ursp);
    for _ in 0..24 {
        if rbp < 0x1000 || rbp > 0x8000_0000_0000 || rbp & 7 != 0 {
            break;
        }
        let (Some(ret), Some(next)) = (rd(rbp + 8), rd(rbp)) else {
            break;
        };
        if ret >= 0x1000 {
            let _ = write!(out, " {:#x}", ret);
        }
        if next <= rbp || next.wrapping_sub(rbp) > 0x20_0000 {
            break;
        }
        rbp = next;
    }
    let _ = write!(out, " spill:");
    for i in 0..24u64 {
        if let Some(w) = rd(ursp + 8 * i) {
            let _ = write!(out, " {:#x}", w);
        }
    }
    Some(out)
}

/// 61st: heap-class pointer test against a task's VMA map (non-exec,
/// non-growdown, canonical userspace). Same rule as the chain walks.
/// `M` is the guard type returned by `vma_try_read` (derefs to the
/// VmaManager).
fn heap_class<M>(va: u64, mgr: &M) -> bool
where
    M: core::ops::Deref<Target = crate::mm::types::VmaManager>,
{
    if va <= 0x10000 || (va >> 47) != 0 {
        return false;
    }
    match mgr.find(crate::mm::types::VirtAddr::new(va as usize)) {
        Some(vma) => {
            !vma.flags().is_executable()
                && !vma.flags().contains(crate::mm::types::VmaFlags::GROWSDOWN)
        }
        None => false,
    }
}

pub fn ffrt_lock_region_scan(tgid: u32, lock: u64) -> alloc::string::String {
    use core::fmt::Write as _;
    use core::sync::atomic::Ordering::Relaxed;
    static DONE: [core::sync::atomic::AtomicU64; 4] =
        [const { core::sync::atomic::AtomicU64::new(0) }; 4];
    let mut slot = DONE[0].load(Relaxed);
    let mut free = false;
    for d in DONE.iter() {
        let v = d.load(Relaxed);
        if v == lock {
            return alloc::string::String::new();
        }
        if v == 0 {
            free = true;
        }
    }
    if !free && slot != 0 {
        return alloc::string::String::new();
    }
    for d in DONE.iter() {
        if d
            .compare_exchange(0, lock, Relaxed, Relaxed)
            .is_ok()
        {
            slot = lock;
            break;
        }
    }
    if slot != lock {
        return alloc::string::String::new();
    }
    let lo = lock.saturating_sub(0x180);
    let hi = lock + 0x28;
    let mut out = alloc::string::String::new();
    let mut scanned_regions = 0usize;
    crate::sched::hotpath::for_each_task_try(|task| {
        if scanned_regions > 0 {
            return;
        }
        // SAFETY: diagnostic reads under the for_each_task contract.
        let tk = crate::sched::r#unsafe::task_shared(task);
        if tk.tgid() as u32 != tgid {
            return;
        }
        let Some(aspace) = tk.address_space() else {
            return;
        };
        let Some(mgr) = aspace.vma_try_read() else {
            return;
        };
        let root = aspace.root_ppn();
        for vma in mgr.iter() {
            if scanned_regions >= 160 {
                break;
            }
            let start = vma.start().as_usize() as u64;
            let end = vma.end().as_usize() as u64;
            let size = end - start;
            // stack-pool class: large anonymous regions.
            // 61st: the floor drops to 0xF0000 — the FFRT coroutine
            // stacks are 1 MB (FFRT_STACK_SIZE 1<<20) mmap'd chunks;
            // the old 4 MB floor swept ONLY the 8 MB OS_IPC thread
            // stacks (whose waiters are the A1 BLOCK_THREAD class) and
            // NEVER the coroutine pool where the M2 HOLDER (A2)
            // suspends.
            if !matches!(vma.vma_type(), crate::mm::types::VmaType::Anonymous)
                || size < 0xF_0000
                || size > 0x20_0_0000
            {
                continue;
            }
            scanned_regions += 1;
            // 61st: sweep EVERY 1MB chunk top (32 KB each) — the FFRT
            // coroutine stack pool packs 1 MB stacks back to back, and
            // a suspended coroutine's active window sits within a few
            // KB BELOW ITS OWN chunk top. The old region-top-only
            // window (last 0x40000) saw only the HIGHEST chunk's
            // coroutine (ohw13-17: always the A1 waiter class) and
            // never the holder's (A2) stack deeper in the pool.
            let mut hits = 0;
            let chunk: u64 = 0x10_0000;
            let win: u64 = 0x8000;
            let mut chunk_top = end;
            while chunk_top > start && hits < 12 {
                let wlo = chunk_top.saturating_sub(win).max(start);
                let mut va = wlo;
                while va < chunk_top && hits < 12 {
                // SAFETY: walker read of the frozen user region.
                let Some(w) = (|| {
                    match rux::sync::r#unsafe::pt_walk(root, va) {
                        Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
                            let kv = crate::arch::mm::memory_layout::phys_to_virt(
                                crate::arch::mm::memory_layout::PhysAddr::new(
                                    (ppn << 12) | (va & 0xfff),
                                ),
                            );
                            // SAFETY: linear-mapped RAM word.
                            Some(rux::sync::r#unsafe::read_volatile_u64(kv.as_usize()))
                        }
                        _ => None,
                    }
                })() else {
                    va += 8;
                    continue;
                };
                if w >= lo && w < hi {
                    // sc57: dump return-address candidates around the
                    // hit (words in executable VMAs) — a waiter's frames
                    // show mutexPrivate::wait/libc++/futex; a suspended
                    // coroutine holding the lock shows its mid-section
                    // call chain (IPC/binder/queue frames).
                    // 61st: ALSO shape-test the window words as
                    // mutexPrivate objects — the suspended critical
                    // section's INNERMOST lock attempt (M3: the
                    // PostTask/handler-queue lock A waits on while
                    // holding THIS lock) spills its pointer in the same
                    // frames. Naming M3 here closes the M2→M3 hop
                    // without another boot.
                    let mut fr = alloc::string::String::new();
                    let mut m3: Option<u64> = None;
                    // window biased ABOVE the hit (older frames — the
                    // guard's callers: the code12/beget path)
                    // 61st: the window covers BOTH directions now — the
                    // frames ABOVE the hit are the outer callers, and the
                    // frames BELOW it (newer, deeper) hold the suspension
                    // site: when the task holds THIS lock and waits on an
                    // INNER mutex (M3 = the PostTask/queue lock), M3's
                    // pointer spill sits below the outer lock()'s rdi
                    // spill. 0x400 down covers the wait/lock frames.
                    for k in -128i64..72 {
                        let fa = (va as i64 + 8 * k - 0x40) as u64;
                        let Some(x) = (|| {
                            match rux::sync::r#unsafe::pt_walk(root, fa) {
                                Some((ppn2, bits2))
                                    if bits2 & 1 != 0 && bits2 & 0x4 != 0 && ppn2 != 0 =>
                                {
                                    let kv2 = crate::arch::mm::memory_layout::phys_to_virt(
                                        crate::arch::mm::memory_layout::PhysAddr::new(
                                            (ppn2 << 12) | (fa & 0xfff),
                                        ),
                                    );
                                    // SAFETY: linear-mapped RAM word.
                                    Some(rux::sync::r#unsafe::read_volatile_u64(kv2.as_usize()))
                                }
                                _ => None,
                            }
                        })() else {
                            continue;
                        };
                        if matches!(
                            mgr.find(crate::mm::types::VirtAddr::new(x as usize)),
                            Some(v2) if v2.flags().is_executable()
                        ) {
                            let _ = write!(fr, " {:#x}", x);
                            continue;
                        }
                        // mutexPrivate shape at x or x-8 (DWARF layout:
                        // vtable +0, l +8, ring head +0x10/+0x18;
                        // ExecuteCtx excluded by the ring-head test).
                        for &b in &[x, x.wrapping_sub(0x8)] {
                            if b == lock || !heap_class(b, &mgr) {
                                continue;
                            }
                            let (Some(vt), Some(lw), Some(pv), Some(nx)) = (
                                rd_word(root, b),
                                rd_word(root, b + 0x8),
                                rd_word(root, b + 0x10),
                                rd_word(root, b + 0x18),
                            ) else {
                                continue;
                            };
                            let l = lw & 0xffff_ffff;
                            if !(l == 1 || l == 2) {
                                continue;
                            }
                            let vt_ok = heap_class(vt, &mgr)
                                && rd_word(root, vt)
                                    .map_or(false, |f| {
                                        matches!(
                                            mgr.find(crate::mm::types::VirtAddr::new(f as usize)),
                                            Some(v3) if v3.flags().is_executable()
                                        )
                                    });
                            let ring_ok = (pv == b + 0x10 && nx == b + 0x10)
                                || (heap_class(nx, &mgr) && rd_word(root, nx) == Some(b + 0x10));
                            if vt_ok && ring_ok && m3.is_none() {
                                m3 = Some(b);
                            }
                        }
                    }
                    let _ = write!(
                        out,
                        " r[{:#x}]+{:#x}(={:#x}) fr:{}",
                        start,
                        va - start,
                        w,
                        fr
                    );
                    if let Some(mb) = m3 {
                        let ml = rd_word(root, mb + 0x8).unwrap_or(0) & 0xffff_ffff;
                        let _ = write!(out, " M3={:#x}l{}", mb, ml);
                        // One holder scan per M3 per boot (the same
                        // stack/region sweep the walk uses for M2).
                        let hs = ffrt_lock_holder_scan(tgid, mb);
                        if !hs.is_empty() {
                            let _ = write!(out, " M3h[{}]", hs);
                        }
                    }
                    hits += 1;
                }
                va += 8;
                }
                chunk_top = chunk_top.saturating_sub(chunk);
            }
        }
    });
    out
}

/// oh67 (67th session): the chain-closed M2 lock object base + the
/// tgid whose chain closed on it — armed once alongside HEAPMAP_ARM;
/// consumed by oh67_m2_decode for the definitive per-round decode.
pub static OH67_M2: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

pub static OH67_M2_TGID: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// oh67: definitive M2 decode. samgr's SystemAbilityManager singleton
/// pointer lives in the executable's BSS (ELF vaddr 0xc7048, symbol
/// _ZN4OHOS20SystemAbilityManager8instanceE); the executable RX
/// segment maps file offset 0x42000 — bias = vma_start - 0x42000.
/// With the singleton base, which MEMBER M2 is resolves by offset
/// against the DWARF member table of this exact image
/// (dBinderServiceLock_@432 shared, abilityMapLock_@524 shared,
/// listenerMapLock_@616, onDemandLock_@728, systemProcessMapLock_@840,
/// startingProcessMapLock_@928, saProfileMapLock_@1104,
/// loadRemoteLock_@1168, saFrequencyLock_@1256 — samgr::mutex ==
/// ffrt::mutexPrivate). Then print l/wlock and walk the waiter ring
/// with the TRUE WaitUntilEntry layout: node{prev@0,next@8}
/// WaitEntry{prev@16,next@24,task@32,wtType@40} WaitUntilEntry
/// {status@44,hasWaitTime@48,tp@56 steady ns,cb@64,wl@112,cv@152}.
/// A non-null task field = a suspended-coroutine waiter (its wake
/// must come from the FFRT scheduler; gid names the dependency);
/// task==null = thread-mode (the looper pool).
/// oh67: pollfd decode for census rows whose (last) syscall is poll —
/// reads the caller's pollfd array through its own page tables (fd:i32,
/// events:i16, revents:i16) and resolves each fd through the task's
/// fdtable to a file path. For the infinite-timeout single-fd parks
/// (hdf_devmgr/audio_server class) this names the fd whose readiness
/// never arrives; for the wide catches (tid345 class) it shows the
/// array's head. Bounded to 6 entries + 6 path lookups.
pub fn oh67_poll_decode(
    task: *mut crate::process::types::Task,
    fds: u64,
    nfds: u64,
    timeout: u64,
) -> alloc::string::String {
    use core::fmt::Write as _;
    if task.is_null() || fds == 0 || nfds == 0 {
        return alloc::string::String::new();
    }
    // SAFETY: diagnostic reads of the audited task under the census
    // contract (the task is pinned by the caller's iteration).
    let tk = crate::sched::r#unsafe::task_shared(task);
    let Some(aspace) = tk.address_space() else {
        return alloc::string::String::new();
    };
    let root = aspace.root_ppn();
    let rd = |va: u64| -> Option<u64> {
        // SAFETY: read-only page-table walk of the task's mm.
        match rux::sync::r#unsafe::pt_walk(root, va) {
            Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
                let kv = crate::arch::mm::memory_layout::phys_to_virt(
                    crate::arch::mm::memory_layout::PhysAddr::new((ppn << 12) | (va & 0xfff)),
                );
                // SAFETY: linear-mapped RAM word.
                Some(rux::sync::r#unsafe::read_volatile_u64(kv.as_usize()))
            }
            _ => None,
        }
    };
    let mut out = alloc::string::String::new();
    let to = timeout as i64;
    let _ = write!(out, " PFD[to={} n={}", to, nfds);
    let n = nfds.min(6) as u64;
    for i in 0..n {
        let Some(w64) = rd(fds + 8 * i) else {
            let _ = write!(out, " unfetched");
            break;
        };
        let fd = w64 as u32 as i32;
        let events = ((w64 >> 32) & 0xffff) as u16;
        let revents = ((w64 >> 48) & 0xffff) as u16;
        if fd < 0 {
            let _ = write!(out, " fd=-");
            continue;
        }
        let file = tk
            .try_fdtable()
            .and_then(|fdt| fdt.get_file(fd as usize));
        let path = file
            .as_ref()
            .map(|f| f.path())
            .unwrap_or_else(|| alloc::string::String::from("closed"));
        let ready = file
            .as_ref()
            .and_then(|f| crate::syscall::support::oh67_fd_ready(f))
            .unwrap_or_default();
        let ops = file
            .as_ref()
            .and_then(|f| f.get_ops().map(|o| o as *const _ as usize))
            .unwrap_or(0);
        let _ = write!(out, " fd={}(0x{:x}/0x{:x}:{})", fd, events, revents, path);
        if !ready.is_empty() {
            let _ = write!(out, " {}", ready);
        }
        let _ = write!(out, " ops={:#x}", ops);
    }
    out.push(']');
    out
}

pub fn oh67_m2_decode() -> alloc::string::String {
    use core::fmt::Write as _;
    use core::sync::atomic::Ordering::Relaxed;
    let m2 = OH67_M2.load(Relaxed);
    let tgid = OH67_M2_TGID.load(Relaxed);
    if m2 == 0 || tgid == 0 {
        return alloc::string::String::new();
    }
    let mut out = alloc::string::String::new();
    let mut done = false;
    crate::sched::hotpath::for_each_task_try(|task| {
        // SAFETY: diagnostic read-only walk under the for_each contract.
        let tk = crate::sched::r#unsafe::task_shared(task);
        if done || tk.tgid() as u32 != tgid {
            return;
        }
        let Some(aspace) = tk.address_space() else {
            return;
        };
        let root = aspace.root_ppn();
        let rd = |va: u64| -> Option<u64> {
            // SAFETY: read-only page-table walk of this task's mm.
            match rux::sync::r#unsafe::pt_walk(root, va) {
                Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
                    let kv = crate::arch::mm::memory_layout::phys_to_virt(
                        crate::arch::mm::memory_layout::PhysAddr::new(
                            (ppn << 12) | (va & 0xfff),
                        ),
                    );
                    // SAFETY: linear-mapped RAM word.
                    Some(rux::sync::r#unsafe::read_volatile_u64(kv.as_usize()))
                }
                _ => None,
            }
        };
        let rd32 = |va: u64| -> Option<u32> {
            rd(va & !7).map(|w| (w >> ((va & 4) * 8)) as u32)
        };
        // 1. executable bias: collect EVERY exec VMA with file offset
        //    0x42000 and length 0x77000 (another lib can coincide); the
        //    right one is the bias whose BSS instance word reads a
        //    canonical user pointer.
        let mut biases: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
        if let Some(mgr) = aspace.vma_try_read() {
            for vma in mgr.iter() {
                if vma.offset() == 0x42000
                    && vma.flags().is_executable()
                    && (vma.end().as_usize() - vma.start().as_usize()) == 0x77000
                {
                    // The RX LOAD is p_offset 0x42000 at p_vaddr
                    // 0x43000 (segments not congruent mod page): the
                    // vma START maps p_vaddr 0x43000, so bias = start -
                    // 0x43000.
                    biases.push(vma.start().as_usize() as u64 - 0x43000);
                }
            }
        }
        let mut bias = 0u64;
        let mut inst = 0u64;
        for b in biases.iter() {
            match rd(b + 0xc7048) {
                Some(v) if v > 0x10000 && (v >> 47) == 0 => {
                    bias = *b;
                    inst = v;
                    break;
                }
                _ => continue,
            }
        }
        if bias == 0 {
            let _ = write!(
                out,
                " OH67[tgid={} biases={:?} inst0={:#x} ilock0={:#x} text0={:#x}]",
                tgid,
                biases,
                biases.first().map(|b| rd(b + 0xc7048).unwrap_or(0)).unwrap_or(0),
                biases
                    .first()
                    .map(|b| rd(b + 0xc7008).unwrap_or(0))
                    .unwrap_or(0),
                biases
                    .first()
                    .map(|b| rd(b + 0x43000).unwrap_or(0))
                    .unwrap_or(0)
            );
        } else {
            let off = m2.wrapping_sub(inst);
        let name = match off {
            432 => "dBinderServiceLock_(shared)",
            524 => "abilityMapLock_(shared)",
            616 => "listenerMapLock_",
            728 => "onDemandLock_",
            840 => "systemProcessMapLock_",
            928 => "startingProcessMapLock_",
            1104 => "saProfileMapLock_",
            1168 => "loadRemoteLock_",
            1256 => "saFrequencyLock_",
            _ => "NOT-A-MEMBER(external-object?)",
        };
        let l = rd32(m2 + 0x8).unwrap_or(0xffff_ffff);
        let wl = rd32(m2 + 0xc).unwrap_or(0xffff_ffff);
        let _ = write!(
            out,
            " OH67[bias={:#x} inst={:#x} m2-inst=+{}({}) l={} wlock={}",
            bias, inst, off, name, l, wl
        );
        }
        // 3. waiter ring walk (true layout): head = m2+0x10,
        //    first = *(m2+0x18), entries link node.next@+8.
        let head = m2 + 0x10;
        let mut cur = rd(m2 + 0x18).unwrap_or(0);
        let mut n = 0u32;
        let mut cotasks: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
        while cur != 0 && cur != head && n < 40 {
            if (cur >> 47) != 0 || cur < 0x1000 {
                let _ = write!(out, " w{}={:#x}BAD", n, cur);
                break;
            }
            let taskp = rd(cur + 0x20).unwrap_or(0);
            let st = rd32(cur + 0x2c).unwrap_or(0xffff);
            let tp = rd(cur + 0x38).unwrap_or(0);
            if taskp != 0 && (taskp >> 47) == 0 && taskp > 0x1000 {
                cotasks.push(taskp);
                let _ = write!(
                    out,
                    " w{}={:#x}TASK(gid={:#x})",
                    n,
                    cur,
                    rd(taskp + 0x20).unwrap_or(0)
                );
            } else {
                let _ = write!(out, " w{}={:#x}thr(st={} tp={})", n, cur, st, tp);
            }
            let nx = rd(cur + 0x8);
            match nx {
                Some(v) if v != 0 && (v == head || ((v >> 47) == 0 && v > 0x1000)) => cur = v,
                _ => {
                    let _ = write!(out, " ring-break@{:#x}", cur);
                    break;
                }
            }
            n += 1;
        }
        let _ = write!(out, " waiters={}", n);
        // 4. suspended-coroutine waiter decode (task != null): TaskBase
        //    {type@24 gid@32 status@48 createTime@96 executeTime@104
        //     fromTid@112} — gid + executeTime stamp the dependency the
        //    M2 waiter graph is stuck on.
        for (i, &ctp) in cotasks.iter().take(4).enumerate() {
            let _ = write!(
                out,
                " ct{}[gid={:#x} type={} st={} cr={} ex={} ftid={}]",
                i,
                rd(ctp + 0x20).unwrap_or(0),
                rd32(ctp + 0x18).unwrap_or(0xffff),
                rd(ctp + 0x30).unwrap_or(0) & 0xff,
                rd(ctp + 0x60).unwrap_or(0),
                rd(ctp + 0x68).unwrap_or(0),
                rd32(ctp + 0x70).unwrap_or(0xffff)
            );
        }
        // First task of the tgid answers the decode once.
        done = true;
    });
    out
}

// === oh68 (68th session): the holder's block primitive ===
//
// The 67th closed the M2 cycle shape but could not name the primitive the
// holder waits on INSIDE the listenerMapLock_ critical section. This
// instrument walks the FFRT TLS chain and the suspended coroutine's saved
// stack. Every offset below is BuildID-verified (libffrt.so bb6920ed,
// gdb DWARF; musl ld-musl-x86_64.so.1 99627cfd, disasm):
//
//   writer thread fs_base (kernel-tracked) -> musl struct pthread
//   tsd pointer @ fs+0x88 (pthread_getspecific: mov 0x88(%fsbase),%rax)
//   -> tsd[g_executeCtxTlsKey] — the key slot is FOUND BY VALIDATION
//   (scan 1..64; the ExecuteCtx's own tid@248 must equal the reading
//   thread's tid), so no libffrt load-bias discovery is needed (the
//   first cut matched libffrt's RX VMA shape and failed — segment skews
//   differ per build).
//   -> ExecuteCtx {vptr@0 qos@8 task@16 wn@32 lastGid_@240 tid@248
//      threadType_@252}; wn (WaitUntilEntry) cv @ ctx+216, so a
//      WaitQueue::ThreadWait park futexes on ctx+224 (musl cond seq
//      word = cv+8 — cond_timedwait lea 0x8(%cond)).
//
//   ctx->task STAYS set while a thread-mode task parks inside Execute()
//   (ThreadWait under wn.cv); a COROUTINE task's ctx->task is cleared the
//   moment it yields (ExecuteTask epilogue), so the holder coroutine is
//   captured at the acquire WRITE itself (fuxwatch_log_write old==0 path)
//   — the only instant the writer provably executes the holder.
//
//   CoTask (432B): TaskBase{type@24 gid@32 status@48 createTime@96
//   executeTime@104 fromTid@112}, func_storage@116, label@216 (libc++
//   string: short = size in byte0>>1, chars at +1; long = size@+8,
//   ptr@+16), we@256{task@280}, wue@296, coRoutine@304, threadMode_@425.
//   CoRoutine (128B): status@0 thEnv@8 task@16 fiber ctx@24 {RBX@+0 RBP@+8
//   R12..R15@+0x10..0x28 RSP@+0x30 RIP@+0x38 -> co+72/co+80}, stkMem@104
//   {size@104 magic@112 stk@120}; the coroutine stack is [co+120,
//   co+120+size) with the saved RSP inside it, growing down.
//
// All reads are page-table walks of frozen wedged memory; rux:fuxstuck (retired debug=fuxstuck)
// gates the print sites (the audit burst and the FW write path).
// x86_64-only (the OH image and the fs/TLS offsets are x86_64).

static OH68_ROOT: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// tgid the cached root belongs to (81st: the park-row decode targets a
/// DIFFERENT process than the OH67-M2 samgr slot — a stale cross-tgid
/// root would read the wrong address space).
static OH68_ROOT_TGID: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// A live CoTask pointer seen by the worker sweep (thread-mode or
/// queued) — the anchor for the slab sweep's derived window (81st).
static OH68_SWEEP_TASK: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// oh68: audit-time decode. Walks EVERY samgr thread's TLS chain (worker
/// sweep — a thread-mode holder keeps ctx->task set while parked inside
/// Execute()), decodes the latched acquire-time holder task (coroutine
/// case), and for each CoTask in the M2 waiter ring prints label +
/// coRoutine saved RSP/RIP (the notify tasks' identities). The owner's
/// saved coroutine stack is walked and every word landing in an exec VMA
/// is printed as a FILE OFFSET — offline addr2line against the identical
/// image .so names the exact cv/future frames.
pub fn oh68_holder_decode() -> alloc::string::String {
    use core::sync::atomic::Ordering::Relaxed;
    // 81st: when no OH67-M2 samgr web exists, decode the latched
    // sync-holder park's process instead (ohw126: foundation tid 852 —
    // the AMS looper parked in FFRT while owing accountmgr's txn; the
    // suspended task and its block primitive live in THAT process).
    let mut tgid = OH67_M2_TGID.load(Relaxed);
    if tgid == 0 {
        tgid = FUXPARK_TGID.load(Relaxed);
    }
    oh68_holder_decode_tgid(tgid)
}

/// 81st: explicit-tgid variant — the census park row decodes ITS OWN
/// process (ohw130: accountmgr tid 1188's park; the OH67_M2 samgr slot
/// would otherwise win the tgid choice on web-forming boots).
pub fn oh68_holder_decode_tgid(tgid: u32) -> alloc::string::String {
    use core::fmt::Write as _;
    use core::sync::atomic::Ordering::Relaxed;
    if tgid == 0 {
        return alloc::string::String::new();
    }
    let mut out = alloc::string::String::new();
    // Pass 1: any thread's aspace root (shared across the tgid). The
    // cache is tgid-scoped: a root latched for another process must not
    // leak into this decode (81st).
    if OH68_ROOT_TGID.load(Relaxed) != tgid {
        OH68_ROOT.store(0, Relaxed);
        OH68_ROOT_TGID.store(tgid, Relaxed);
    }
    crate::sched::hotpath::for_each_task_try(|task| {
        if OH68_ROOT.load(Relaxed) != 0 {
            return;
        }
        // SAFETY: diagnostic read under the for_each_task contract.
        let tk = crate::sched::r#unsafe::task_shared(task);
        if tk.tgid() as u32 != tgid {
            return;
        }
        if let Some(aspace) = tk.address_space() {
            OH68_ROOT.store(aspace.root_ppn(), Relaxed);
        }
    });
    let root = OH68_ROOT.load(Relaxed);
    if root == 0 {
        return alloc::string::String::new();
    }
    let key = OH68_KEY.load(Relaxed);
    let rd = |va: u64| -> Option<u64> { rd_word(root, va) };
    let rd32 = |va: u64| -> Option<u32> { rd(va & !7).map(|w| (w >> ((va & 4) * 8)) as u32) };
    let _ = write!(out, " OH68[tgid={} key={}", tgid, key);

    // Exec-VMA table for module-classification of stack words:
    // (start, end, file_off_at_start, name). ONE task's aspace (all
    // threads of the tgid share it — walking every thread duplicated the
    // table and burned the cap on dupes before reaching libffrt). Cap 48
    // (libipc_single/libdbinder sit past the old 24 cap and their frames
    // are the divergence evidence); prints once per decode.
    let mut exec_vmas: alloc::vec::Vec<(u64, u64, u64, alloc::string::String)> =
        alloc::vec::Vec::new();
    if root != 0 {
        crate::sched::hotpath::for_each_task_try(|task| {
            if !exec_vmas.is_empty() || exec_vmas.len() >= 48 {
                return;
            }
            // SAFETY: diagnostic read under the for_each_task contract.
            let tk = crate::sched::r#unsafe::task_shared(task);
            if tk.tgid() as u32 != tgid {
                return;
            }
            if let Some(aspace) = tk.address_space() {
                if let Some(mgr) = aspace.vma_try_read() {
                    for vma in mgr.iter() {
                        if vma.flags().is_executable() && exec_vmas.len() < 48 {
                            let name = aspace
                                .try_get_vma_file(vma.start().as_usize())
                                .map(|f| f.path())
                                .unwrap_or_default();
                            // Keep only the basename (bounded).
                            let base: alloc::string::String = name
                                .rsplit('/')
                                .next()
                                .unwrap_or("-")
                                .chars()
                                .take(24)
                                .collect();
                            exec_vmas.push((
                                vma.start().as_usize() as u64,
                                vma.end().as_usize() as u64,
                                vma.offset() as u64,
                                base,
                            ));
                        }
                    }
                }
            }
        });
    }
    for (i, &(s, e, f, ref n)) in exec_vmas.iter().enumerate() {
        let _ = write!(out, " v{}={:#x}-{:#x}/{:#x}/{}", i, s, e, f, n);
    }
    let word_off = |w: u64| -> Option<(usize, u64)> {
        for (i, &(s, e, f, _)) in exec_vmas.iter().enumerate() {
            if w >= s && w < e {
                return Some((i, w - s + f));
            }
        }
        None
    };

    // Worker sweep: every thread's ctx->task. Thread-mode holders show
    // task != 0; the park word for a ThreadWait is ctx+224 (wn.cv+8).
    let mut swept = 0u32;
    crate::sched::hotpath::for_each_task_try(|task| {
        if swept >= 24 {
            return;
        }
        // SAFETY: diagnostic read under the for_each_task contract.
        let tk = crate::sched::r#unsafe::task_shared(task);
        if tk.tgid() as u32 != tgid {
            return;
        }
        swept += 1;
        let tid = tk.pid() as u32;
        let fs = oh68_fs_base(tk);
        let ctx = oh68_ctx_of(root, fs, tid);
        if ctx == 0 {
            return;
        }
        let taskp = rd(ctx + 16).unwrap_or(0);
        let _ = write!(
            out,
            " w{}[ctx={:#x} task={:#x} gid={:#x} ty={} st={} ft={}",
            tid,
            ctx,
            taskp,
            if taskp > 0x1000 {
                rd(taskp + 0x20).unwrap_or(0)
            } else {
                0
            },
            if taskp > 0x1000 {
                rd32(taskp + 0x18).unwrap_or(0xffff)
            } else {
                0xffff
            },
            if taskp > 0x1000 {
                rd(taskp + 0x30).unwrap_or(0) & 0xff
            } else {
                0xff
            },
            if taskp > 0x1000 {
                rd32(taskp + 0x70).unwrap_or(0xffff)
            } else {
                0xffff
            }
        );
        // 81st: for thread-mode entries (task set — the thread parks
        // inside Execute()) also print the task LABEL (same SSO decode
        // as the cotask helper, long form included) and the ThreadWait
        // park word ctx+224, so a standing park row's uaddr correlates
        // with its FFRT task by construction instead of needing the
        // acquire latch.
        if taskp > 0x1000 && (taskp >> 47) == 0 {
            OH68_SWEEP_TASK.store(taskp, Relaxed);
            let w0 = rd(taskp + 0xd8).unwrap_or(0);
            let mut lbl: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
            if w0 & 1 == 0 {
                let n = ((w0 & 0xff) >> 1) as usize;
                if n > 0 && n <= 22 {
                    let w1 = rd(taskp + 0xd8 + 8).unwrap_or(0);
                    let w2 = rd(taskp + 0xd8 + 16).unwrap_or(0);
                    let buf = [w0, w1, w2];
                    for i in 1..=n {
                        let b = ((buf[i / 8] >> (8 * (i % 8))) & 0xff) as u8;
                        lbl.push(b);
                    }
                }
            } else {
                let size = rd(taskp + 0xd8 + 8).unwrap_or(0);
                let ptr = rd(taskp + 0xd8 + 16).unwrap_or(0);
                if size <= 64 && (ptr >> 47) == 0 && ptr > 0x1000 {
                    for i in 0..size as u64 {
                        let b = rd(ptr + i).map(|w| (w & 0xff) as u8).unwrap_or(0);
                        lbl.push(b);
                    }
                }
            }
            let _ = write!(
                out,
                " lbl=\"{}\" parkw={:#x}",
                core::str::from_utf8(&lbl).unwrap_or("?"),
                rd(ctx + 224).unwrap_or(0)
            );
        }
        let _ = write!(out, "]");
    });
    let _ = write!(out, " swept={}", swept);

    // Deep decode of one CoTask: base fields + label + wue/coRoutine/
    // threadMode + saved fiber RIP/RSP with their module file offsets.
    let cotask = |tp: u64,
                  tag: &str,
                  deep: bool,
                  out: &mut alloc::string::String| {
        if tp == 0 || (tp >> 47) != 0 || tp < 0x1000 {
            return;
        }
        let _ = write!(
            out,
            " {}[tp={:#x} gid={:#x} ty={} st={} ct={} ex={} ft={} wue={:#x} thrM={}",
            tag,
            tp,
            rd(tp + 0x20).unwrap_or(0),
            rd32(tp + 0x18).unwrap_or(0xffff),
            rd(tp + 0x30).unwrap_or(0) & 0xff,
            rd(tp + 0x60).unwrap_or(0),
            rd(tp + 0x68).unwrap_or(0),
            rd32(tp + 0x70).unwrap_or(0xffff),
            rd(tp + 0x128).unwrap_or(0),
            rd(tp + 0x1a9).map(|v| v & 0xff).unwrap_or(0xff)
        );
        // label (libc++ string @+216 = 0xd8; 24-byte SSO buffer spans
        // three words; long form: size@+8, ptr@+16).
        let w0 = rd(tp + 0xd8).unwrap_or(0);
        let mut lbl: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        if w0 & 1 == 0 {
            let n = ((w0 & 0xff) >> 1) as usize;
            if n > 0 && n <= 22 {
                let w1 = rd(tp + 0xd8 + 8).unwrap_or(0);
                let w2 = rd(tp + 0xd8 + 16).unwrap_or(0);
                let buf = [w0, w1, w2];
                for i in 1..=n {
                    let b = ((buf[i / 8] >> (8 * (i % 8))) & 0xff) as u8;
                    lbl.push(b);
                }
            }
        } else {
            let size = rd(tp + 0xd8 + 8).unwrap_or(0);
            let ptr = rd(tp + 0xd8 + 16).unwrap_or(0);
            if size <= 64 && (ptr >> 47) == 0 && ptr > 0x1000 {
                for i in 0..size as u64 {
                    let b = rd(ptr + i).map(|w| (w & 0xff) as u8).unwrap_or(0);
                    lbl.push(b);
                }
            }
        }
        let _ = write!(out, " lbl=\"{}\"", core::str::from_utf8(&lbl).unwrap_or("?"));
        // saved coroutine context (coRoutine@304 = 0x130)
        let co = rd(tp + 0x130).unwrap_or(0);
        if co != 0 && (co >> 47) == 0 && co > 0x1000 {
            let rsp = rd(co + 24 + 48).unwrap_or(0);
            let rip = rd(co + 24 + 56).unwrap_or(0);
            let stk_sz = rd(co + 0x68).unwrap_or(0);
            let ripoff = match word_off(rip) {
                Some((m, o)) => alloc::format!("v{}+{:#x}", m, o),
                None => alloc::string::String::from("?"),
            };
            let _ = write!(
                out,
                " co={:#x}(st={} rsp={:#x} rip={:#x}@{} sz={:#x})",
                co,
                rd32(co).unwrap_or(0xffff),
                rsp,
                rip,
                ripoff,
                stk_sz
            );
            let lo = co + 120;
            let hi = co + 120 + stk_sz.min(0x100000);
            if rsp >= lo && rsp < hi && (rsp & 7) == 0 {
                // Walk the suspended coroutine stack upward (older
                // frames); every word inside an exec VMA is a candidate
                // return address — print MODULE INDEX + FILE OFFSET for
                // offline addr2line. Deep: 6 KB / 40 prints; ring rows:
                // 12 / 2 KB.
                let (cap, win) = if deep { (40, 6144) } else { (12, 2048) };
                let mut found = 0u32;
                let mut va = rsp;
                let top = (rsp + win).min(hi);
                let _ = write!(out, " stk[");
                while va < top && found < cap {
                    if let Some(w) = rd(va) {
                        if let Some((m, off)) = word_off(w) {
                            let _ = write!(out, " v{}:{:#x}", m, off);
                            found += 1;
                        }
                    }
                    va += 8;
                }
                let _ = write!(out, "]");
            }
        }
    };

    // The latched acquire-time holder (coroutine case) — deep walk.
    let otask = FUXWATCH_OWNER_TASK.load(Relaxed);
    let octx = OH68_OWNER_CTX.load(Relaxed);
    let parkw = if octx != 0 { octx + 224 } else { 0 };
    let _ = write!(out, " owner[task={:#x} ctx={:#x} parkw={:#x}]", otask, octx, parkw);
    // oh68b probe: if the latched holder's saved RIP is not covered by
    // any EXEC vma, name the containing vma's recorded flags (an
    // exec-bit bookkeeping loss shows here; the 24-entry tables of
    // ohw71/72 lacked the rip's module entirely).
    if otask != 0 {
        if let Some(co) = rd(otask + 0x130) {
            if co > 0x1000 && (co >> 47) == 0 {
                if let Some(rip) = rd(co + 24 + 56) {
                    if word_off(rip).is_none() {
                        let mut named = false;
                        crate::sched::hotpath::for_each_task_try(|task| {
                            if named {
                                return;
                            }
                            // SAFETY: diagnostic read under for_each.
                            let tk = crate::sched::r#unsafe::task_shared(task);
                            if tk.tgid() as u32 != tgid {
                                return;
                            }
                            if let Some(aspace) = tk.address_space() {
                                if let Some(mgr) = aspace.vma_try_read() {
                                    if let Some(v) = mgr.find(crate::mm::types::VirtAddr::new(
                                        rip as usize,
                                    )) {
                                        let name = aspace
                                            .try_get_vma_file(v.start().as_usize())
                                            .map(|f| f.path())
                                            .unwrap_or_default();
                                        let _ = write!(
                                            out,
                                            " RIPVMA[{}-{} off={:#x} flags={:#x} {}]",
                                            v.start().as_usize(),
                                            v.end().as_usize(),
                                            v.offset(),
                                            v.flags().bits(),
                                            name
                                        );
                                        named = true;
                                    }
                                }
                            }
                        });
                        if !named {
                            let _ = write!(out, " RIPVMA[none]");
                        }
                    }
                }
            }
        }
    }
    if otask != 0 {
        cotask(otask, "own", true, &mut out);
    }

    // The M2 ring's CoTask waiters (the notify tasks queued behind M2):
    // label + saved RIP names them without a full walk. Entry == the
    // task's own wue (heap WaitUntilEntry) marks the coroutine waiters;
    // thread parks (entry == some ctx+32) are covered by the sweep.
    let m2 = OH67_M2.load(Relaxed);
    if m2 != 0 {
        let head = m2 + 0x10;
        let mut cur = rd(m2 + 0x18).unwrap_or(0);
        let mut n = 0u32;
        let mut ct_n = 0u32;
        while cur != 0 && cur != head && n < 40 && ct_n < 4 {
            if (cur >> 47) != 0 || cur < 0x1000 {
                break;
            }
            let taskp = rd(cur + 0x20).unwrap_or(0);
            if taskp != 0 && (taskp >> 47) == 0 && taskp > 0x1000 {
                // Coroutine waiter on this mutex: the ring entry IS the
                // task's own we (CoTask+256 = +0x100). CV waiters sit at
                // task->wue (+0x128). Thread parks (entry == some ctx+32)
                // are covered by the sweep.
                if rd(taskp + 0x100) == Some(cur) || rd(taskp + 0x128) == Some(cur) {
                    cotask(taskp, "rw", false, &mut out);
                    ct_n += 1;
                }
            }
            match rd(cur + 0x8) {
                Some(v) if v != 0 && (v == head || ((v >> 47) == 0 && v > 0x1000)) => cur = v,
                _ => break,
            }
            n += 1;
        }
    }

    // 78th: CoTask slab sweep. The wedge's HOLDER (the suspended
    // mid-run task that acquired the processLock and CoWait'ed inside
    // the critical section) is invisible to everything above: it is
    // not in the mutex waiter ring (holders are not waiters), its
    // worker's ctx->task was cleared at the yield (the worker sweep
    // only sees thread-mode or queued tasks), and the fuxwatch owner
    // latch needs the watch armed BEFORE the acquire — the chain arms
    // at the first stuck print, which is AFTER the founder acquired
    // (ohw115: holder acquired pre-81.5 s, watch armed 84 s). A CoTask
    // whose saved coroutine RIP sits inside libffrt's CoWait internals
    // (vaddr class 0x87xxx-0x8axxx file-offset window — NOT
    // context_entry, the fresh-queue state) is a suspended mid-run
    // task; its saved stack names the critical-section site directly.
    // Sweep the CoTask heap neighborhood (observed 0x7ff0004cxx-
    // 0x7ff0005xxxx, 432-byte CoTask stride with neighbors at any
    // 8-byte offset), shape-test each candidate, print up to 24 with
    // label + saved rip/stack. Only runs when the M2 latch is set
    // (wedge live) and at most once per 30 s.
    {
        static SLAB_LAST_J: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let nowj = crate::timer::get_jiffies() as u32;
        let lj = SLAB_LAST_J.load(Relaxed);
        if nowj.wrapping_sub(lj) >= 3000 {
            SLAB_LAST_J.store(nowj, Relaxed);
            let _ = write!(out, " SLAB[");
            let mut printed = 0u32;
            let mut scanned = 0u64;
            // 81st: the window is DERIVED, not fixed — ohw131's wedged
            // process (accountmgr-class) holds CoTasks at 0x7ff085xxxxxx,
            // far outside the samgr-era 0x7ff0004c-0x7ff00060 window the
            // 78th hardcoded (its SLAB printed empty, scanned=163840).
            // Anchor: any task pointer this decode already knows (a
            // sweep thread-mode task, the acquire latch, or the M2 ring
            // waiters); fall back to the old fixed window.
            let mut anchor = OH68_SWEEP_TASK.load(Relaxed);
            if anchor == 0 {
                anchor = FUXWATCH_OWNER_TASK.load(Relaxed);
            }
            if anchor == 0 {
                // The parked word itself lives in the wedged process's
                // heap — CoTasks from the same arena are often within
                // the window.
                anchor = FUXPARK_WORD.load(Relaxed) & !0xfff;
            }
            if anchor == 0 {
                anchor = OH67_M2.load(Relaxed);
            }
            // 81st fix: scan BOTH the derived anchor window AND the
            // legacy fixed window — when the anchor is the park word but
            // the CoTasks live in the samgr-era region (ohw136: word
            // 0x7ff000bcd5b4 vs CoTasks at 0x7ff0004c-0x7ff00060), the
            // derived-only window missed them exactly as the fixed-only
            // window missed the accountmgr class.
            let (win_lo, win_hi) = if anchor > 0x1000 && (anchor >> 47) == 0 {
                ((anchor - 0x100000).max(0x7ff000000000), anchor + 0x100000)
            } else {
                (0x7ff0004c0000u64, 0x7ff000600000u64)
            };
            let legacy = (0x7ff0004c0000u64, 0x7ff000600000u64);
            let mut wins = [(win_lo, win_hi), legacy];
            if (win_lo, win_hi) == legacy {
                wins = [legacy, (0, 0)];
            }
            // libffrt's RX segment covers the CoWait internals; the
            // context_entry trampoline (fresh, never-started coroutine)
            // is at file offset 0x86a24 on this image — a saved rip AT
            // it means queued, not suspended.
            'outer: for &(lo, hi) in wins.iter() {
            if lo == 0 {
                break;
            }
            for base in (lo..hi).step_by(8) {
                scanned += 1;
                if scanned > 400_000 {
                    break;
                }
                let w0 = match rd(base + 0xd8) {
                    Some(v) => v,
                    None => continue,
                };
                // SSO libc++ string at +0xd8 with a plausible length and
                // printable ASCII in the first 8 bytes (labels observed:
                // "sq_collect_6_PostTask_488" class).
                if w0 & 1 != 0 {
                    continue; // long-form label: rare; skip (ptr deref cost)
                }
                let n = ((w0 & 0xff) >> 1) as usize;
                if !(6..=22).contains(&n) {
                    continue;
                }
                let b0 = (w0 >> 8) & 0xff;
                let b1 = (w0 >> 16) & 0xff;
                let b2 = (w0 >> 24) & 0xff;
                // 81st: sq_ (serial-queue) OR any printable 6..22-char
                // label — the accountmgr/foundation classes name their
                // tasks differently; the shape tests below do the real
                // filtering.
                if !(0x20..0x7f).contains(&b0)
                    || !(0x20..0x7f).contains(&b1)
                    || !(0x20..0x7f).contains(&b2)
                {
                    continue;
                }
                // gid/ty/st plausibility (fields observed: gid<=0x1000,
                // ty in {2,3,4}, st in 1..=8).
                let gid = match rd(base + 0x20) {
                    Some(g) if (1..0x10000).contains(&g) => g,
                    _ => continue,
                };
                let ty = rd32(base + 0x18).unwrap_or(0xffff);
                if ty > 4 {
                    continue;
                }
                let st = rd(base + 0x30).unwrap_or(0) & 0xff;
                if !(1..=8).contains(&st) {
                    continue;
                }
                // coRoutine: a live fiber with a saved rip inside an exec
                // VMA.
                let co = match rd(base + 0x130) {
                    Some(c) if c > 0x1000 && (c >> 47) == 0 => c,
                    _ => continue,
                };
                let rsp = match rd(co + 24 + 48) {
                    Some(v) if v > 0x1000 && (v >> 47) == 0 => v,
                    _ => continue,
                };
                let rip = match rd(co + 24 + 56) {
                    Some(v) => v,
                    None => continue,
                };
                let ripoff = match word_off(rip) {
                    Some((m, o)) => alloc::format!("v{}+{:#x}", m, o),
                    None => continue,
                };
                // Label body (bytes 4..n of the SSO window span w0/w1/w2).
                let w1 = rd(base + 0xd8 + 8).unwrap_or(0);
                let w2 = rd(base + 0xd8 + 16).unwrap_or(0);
                let buf = [w0, w1, w2];
                let mut lbl: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
                for i in 1..=n {
                    let b = ((buf[i / 8] >> (8 * (i % 8))) & 0xff) as u8;
                    if !(0x20..0x7f).contains(&b) {
                        lbl.clear();
                        break;
                    }
                    lbl.push(b);
                }
                if lbl.is_empty() {
                    continue;
                }
                // Deep stack walk for the first few SUSPENDED mid-run
                // tasks (rip NOT at context_entry): the frames above the
                // CoWait name the in-critical-section site.
                let _ = write!(
                    out,
                    " {}[tp={:#x} g{} ty{} st{} rip={} \"{}\"]",
                    if printed == 0 { "" } else { "," },
                    base,
                    gid,
                    ty,
                    st,
                    ripoff,
                    core::str::from_utf8(&lbl).unwrap_or("?")
                );
                printed += 1;
                if printed <= 2 {
                    // Stack walk for the FIRST TWO matches (any module —
                    // the libffrt index shifts per boot; ohw119 had it
                    // at v47 and the hardcoded v19 gate suppressed every
                    // walk): saved rsp upward, exec-VMA words only. The
                    // frames above the CoWait name the samgr function
                    // that holds/waits the lock.
                    let stk_sz = rd(co + 0x68).unwrap_or(0).min(0x1800);
                    let hi = co + 120 + stk_sz;
                    if rsp >= co + 120 && rsp < hi && (rsp & 7) == 0 {
                        let _ = write!(out, " stk[");
                        let mut found = 0u32;
                        let mut va = rsp;
                        while va < hi && found < 16 {
                            if let Some(w) = rd(va) {
                                if let Some((m, o)) = word_off(w) {
                                    let _ = write!(out, " v{}:{:#x}", m, o);
                                    found += 1;
                                }
                            }
                            va += 8;
                        }
                        let _ = write!(out, "]");
                    }
                }
                if printed >= 24 {
                    break 'outer;
                }
            }
            }
            let _ = write!(out, " ] scanned={}", scanned);
        }
    }
    out
}

// ==== 82nd: SQWALK — the SerialQueue/QueueTask walk ====
//
// Why: musl rwlock readers are ANONYMOUS (count only, no list), so the
// 16 read-holders freezing every writer cannot be named from the lock.
// They are FFRT coroutines that yielded inside the read section; their
// stacks live in the heap and are reachable only through the task
// graph. The 81st's slab sweep printed empty on ohw127-139 because its
// shape test REQUIRED a short-form SSO label at +0xd8 — QueueTask
// labels are queue-name[_attr-name]_gid and routinely exceed the
// 22-char SSO window (long form, heap pointer). The sqwalk shape test
// is STRUCTURAL instead:
//   type@24 == ffrt_queue_task(3), gid@32 small, curStatus@49 valid,
//   handler_@440 -> queue_@8 -> BaseQueue validated by dequeFunc_@128
//   (a code pointer inside libffrt's EXEC vma) + printable name_@200.
// For every validated QueueTask AND every task referenced by its queue
// (whenMap_ tree, headTaskVec_ batch, nextTask_ chain, handler
// curTaskVec_), decode identity + saved coroutine context, walk the
// saved stack, and MATCH IT AGAINST THE FROZEN LOCK WORD: a reader
// suspended inside its critical section keeps the rwlock pointer in
// its saved stack.
//
// Layouts (libffrt bb6920ed; image copy and the unstripped out-tree
// twin verified same BuildID — ptype /o, 82nd):
//   BaseQueue(224)/SerialQueue(232): vptr@0, mutex_@8(64),
//     queueId_@72(u32), delayStatus_@76, isExit_@77,
//     isActiveState_@78, whenMap_@80 (libc++ __tree: begin_node@80,
//     root@88, size@96; node: left@0 right@8 parent@16 color@24,
//     value key@32 task@40), headTaskVec_@104 (begin@104 end@112
//     cap@120), dequeFunc_@128, cond_@136(64), name_@200(24).
//   QueueTask(496) = CoTask(432) + uptime_@432, handler_@440,
//     insertHead_@448, delay_@456, schedTimeout_@464, nextTask_@472,
//     isFinished_@480, onWait_@481, isWeStart_@482, prio_@484,
//     monitorWe_@488.
//   QueueHandler(320): qos_@0, queue_@8, isUsed_@16, execTaskId_@24,
//     maxConcurrency_@32, curTaskVec_@40 (begin@40 end@48), ...,
//     we_@264, threadMode_@316.

static SQW_TGID: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

static SQW_WORD: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

static SQW_CURSOR: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

static SQW_HI: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

static SQW_DONE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

static SQW_LAST_J: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// root_ppn of the pass's mm (83rd: drained-word abandonment reads the
/// OPEN word through it — a transient reader pile must not lock the
/// wedge's own word out for the whole boot, ohw148's trap).
static SQW_ROOT: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// 83rd: RAW phase cursor + done latch — after the shape pass, a second
/// full-window scan looks for the lock word VALUE itself. ohw149's
/// foundation pass validated ZERO tasks (no shape hit anywhere in
/// 146 MB) while the lock sits frozen at 16 readers: if a holder keeps
/// the pointer in a register-save area or a stack buffer whose owning
/// task fails the shape test, only the raw value scan finds it — and
/// if the value appears NOWHERE in userspace, the reader COUNT is
/// leaked (abandoned coroutines), not held.
static SQW_RAWCUR: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

static SQW_RAWDONE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// RAW-phase hits printed (cap across the whole phase).
static SQW_RAWHITS: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// 83rd: set when the region table hits its cap (truncation visibility).
static SQW_TRUNC: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// 83rd: the open pass was armed from a deep CLUSTER (non-rwlock word);
/// such a pass never drains on value shape.
static SQW_DEEP: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// Mapped non-exec RW regions (the scan window): starts/ends pairs,
/// count in SQW_RN. Rebuilt per (tgid, word) burst pass.
static SQW_RSTARTS: [core::sync::atomic::AtomicU64; 512] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 512];

static SQW_RENDS: [core::sync::atomic::AtomicU64; 512] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 512];

static SQW_RN: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

const SQW_LO: u64 = 0x7ff0_0000_0000;

/// Words per burst (83rd: 500k -> 1M -> 4M — the 594 MB full window is
/// 74M words; at 1M/burst the pass needed 74 gated rounds and could
/// not finish before the boot's own watchdog death. In the censusd
/// kthread a multi-second burst only delays census prints, so 4M
/// words/burst completes a full dual-arena pass in <= 19 rounds).
const SQW_BUDGET: u64 = 4_000_000;

/// Hard window cap (bytes) above SQW_LO — the OH heap arena observed on
/// every wedged process sits far below this.
const SQW_MAX_WIN: u64 = 0x1000_0000;

fn sqw_canon(p: u64) -> bool {
    p > 0x1000 && (p >> 47) == 0
}

/// One sqwalk burst: scans up to SQW_BUDGET words from the cursor,
/// validating QueueTask candidates structurally and printing every
/// hit (task decode + queue summary + saved-stack lock-word match).
/// 83rd: split into ARM (cheap, callable from ANY context — softirq,
/// kthread: gates + sticky-word arbitration + pass OPEN, one page walk
/// worst case) and SLICE (the actual 1M-word burst — kthread ONLY:
/// ohw151 deadlocked both CPUs when a 594 MB window's bursts ran in
/// the timer softirq: the softirq-context locks stayed held across
/// seconds of page walks and every other CPU spun on them forever).
/// Sliced across bursts until the window is covered, then the RAW
/// phase, then latched. Prints through ConsoleWriter.
pub fn sqwalk_step(tgid: u32, word: u64) {
    sqwalk_step_deep(tgid, word, false);
}

/// `deep`: armed from a >=12-thread futex cluster on a NON-rwlock word
/// (ohw152: 16 samgr threads never-woken on a cond-class ...194 word,
/// the same never-sent stall without any 0x80000010 face forming). The
/// raw phase then maps that word's holder graph in the host tgid.
pub fn sqwalk_step_deep(tgid: u32, word: u64, deep: bool) {
    use core::fmt::Write as _;
    use core::sync::atomic::Ordering::Relaxed;
    if cfg!(not(target_arch = "x86_64")) || tgid == 0 || !sqw_canon(word) {
        return;
    }
    if !crate::trace::EVENT_FUXSTUCK.armed() {
        return;
    }
    // Sticky word (ohw145 lesson: THREE deep-frozen words in one boot
    // — foundation twins + systemui — and every trigger's word reset
    // the cursor; 20 slices burned without one completing a pass).
    // Once a pass starts, different words wait until it completes —
    // EXCEPT when the open word has DRAINED (83rd, the ohw148 trap: a
    // TRANSIENT 16-reader pile in tgid 650 opened the pass, drained to
    // readers=1 within 35 ms, and then starved the real wedge word
    // 549/0x7ff00081391c for the whole boot). Read the open word via
    // the pass's own root: no writer marker and < 8 readers = not
    // frozen anymore — abandon the pass and let the new word in. A
    // DEEP-cluster pass never drains on value shape (a cond word's
    // value says nothing about liveness); passes complete reliably now
    // (kthread-driven full window), so stickiness is bounded anyway.
    if SQW_DONE.load(Relaxed) == 0
        && SQW_WORD.load(Relaxed) != 0
        && (SQW_WORD.load(Relaxed) != word || SQW_TGID.load(Relaxed) != tgid)
    {
        let old_w = SQW_WORD.load(Relaxed);
        let old_root = SQW_ROOT.load(Relaxed);
        let drained = match rd_word(old_root, old_w & !7) {
            Some(x) => {
                let v = ((x >> ((old_w & 4) * 8)) & 0xffff_ffff) as u32;
                if SQW_DEEP.load(Relaxed) != 0 {
                    true
                } else {
                    // hw1 (huks-wedge): the rwlock-shape test (bit31 set,
                    // 8+ "readers") called live musl-MUTEX freezes
                    // "drained": the frozen pthread-mutex words sit at
                    // 0x2 / 0x10 with no bit31 (hw1w1 FW decode: acquire
                    // CAS musl+0x11b8d7 0->0x10, release musl+0x11bd83;
                    // huks's -224 family parks at val 0x2), and the
                    // bit31 marker flickers with waiter churn on the
                    // 0x80000010 face. A lock of either family is FREE
                    // only when the whole word reads 0 — a cycling word
                    // drains at its next unlock, a frozen one never.
                    v == 0
                }
            }
            None => true, // mm gone: nothing to scan anyway
        };
        if !drained {
            return;
        }
        let mut dw = crate::debug::types::ConsoleWriter::new();
        use core::fmt::Write as _;
        let _ = writeln!(
            dw,
            "SQW-DRAIN abandoning tgid={} word={:#x} (lock no longer frozen) for tgid={} word={:#x}",
            SQW_TGID.load(Relaxed),
            old_w,
            tgid,
            word
        );
        SQW_WORD.store(0, Relaxed); // pass the sticky guard below
    }
    if SQW_TGID.load(Relaxed) != tgid || SQW_WORD.load(Relaxed) != word {
        SQW_TGID.store(tgid, Relaxed);
        SQW_WORD.store(word, Relaxed);
        SQW_CURSOR.store(SQW_LO, Relaxed);
        SQW_HI.store(0, Relaxed);
        SQW_DONE.store(0, Relaxed);
        SQW_RN.store(0, Relaxed);
        SQW_ROOT.store(0, Relaxed);
        SQW_RAWCUR.store(SQW_LO, Relaxed);
        SQW_RAWDONE.store(0, Relaxed);
        SQW_RAWHITS.store(0, Relaxed);
        SQW_TRUNC.store(0, Relaxed);
        SQW_DEEP.store(if deep { 1 } else { 0 }, Relaxed);
    }
}

/// The slice half of the walk: runs at most ONE burst (shape or raw
/// phase) of the OPEN pass. kthread context ONLY (censusd) — the
/// seconds-class page-walk loop must never run under softirq locks.
pub fn sqwalk_slice() {
    use core::fmt::Write as _;
    use core::sync::atomic::Ordering::Relaxed;
    if cfg!(not(target_arch = "x86_64")) {
        return;
    }
    if !crate::trace::EVENT_FUXSTUCK.armed() {
        return;
    }
    if SQW_RAWDONE.load(Relaxed) != 0 || SQW_WORD.load(Relaxed) == 0 {
        return;
    }
    let tgid = SQW_TGID.load(Relaxed);
    let word = SQW_WORD.load(Relaxed);
    if tgid == 0 || !sqw_canon(word) {
        return;
    }
    let nowj = crate::timer::get_jiffies() as u32;
    let lj = SQW_LAST_J.load(Relaxed);
    if nowj.wrapping_sub(lj) < 50 && SQW_CURSOR.load(Relaxed) != SQW_LO {
        return; // mid-slice: one burst per ~2 censusd rounds (250 ms)
    }
    SQW_LAST_J.store(nowj, Relaxed);
    // Root + exec-VMA table (module classification + dequeFunc
    // validation) + the mapped-REGION table (the scan window: the OH
    // layout splits into a LOW early arena at 0x7ff000xxxxxx AND high
    // mmap chunks at 0x7ff08xxxxxxx 2 GB up — foundation's QueueTasks
    // live at 0x7ff08667xxxx, far past any linear window from
    // SQW_LO). Rebuilt per burst: ~100 vma reads, noise against the
    // 500k-word slice.
    let mut root = 0u64;
    let mut hi_hint = 0u64;
    let mut exec_vmas: alloc::vec::Vec<(u64, u64, u64, alloc::string::String)> =
        alloc::vec::Vec::new();
    // non-exec RW regions, (start,end), ascending insertion
    let mut regions: alloc::vec::Vec<(u64, u64)> = alloc::vec::Vec::new();
    crate::sched::hotpath::for_each_task_try(|task| {
        // SAFETY: diagnostic read under the for_each_task contract.
        let tk = crate::sched::r#unsafe::task_shared(task);
        if root != 0 || tk.tgid() as u32 != tgid {
            return;
        }
        if let Some(aspace) = tk.address_space() {
            root = aspace.root_ppn();
            if let Some(mgr) = aspace.vma_try_read() {
                for vma in mgr.iter() {
                    let vs = vma.start().as_usize() as u64;
                    let ve = vma.end().as_usize() as u64;
                    if ve > hi_hint {
                        hi_hint = ve;
                    }
                    if vma.flags().is_executable() {
                        if exec_vmas.len() < 200 {
                            let name = aspace
                                .try_get_vma_file(vma.start().as_usize())
                                .map(|f| f.path())
                                .unwrap_or_default();
                            let base: alloc::string::String = name
                                .rsplit('/')
                                .next()
                                .unwrap_or("-")
                                .chars()
                                .take(24)
                                .collect();
                            exec_vmas.push((vs, ve, vma.offset() as u64, base));
                        }
                    } else if regions.len() < 512
                        && ve > vs
                        && ve - vs <= SQW_MAX_WIN
                        && vs >= SQW_LO
                    {
                        // merge with a previous region if adjacent
                        // (within 64 KB) to keep the table small
                        if let Some(last) = regions.last_mut() {
                            if vs <= last.1 + 0x1_0000 {
                                if ve > last.1 {
                                    last.1 = ve;
                                }
                                continue;
                            }
                        }
                        regions.push((vs, ve));
                    }
                }
            }
        }
        if regions.len() >= 512 {
            // 83rd: truncation MUST be visible — the 82nd's 128-cap
            // silently dropped foundation's high heap chunks (0x7ff08cbf+
            // / 0x7ff08f0d+ where the FFRT task stacks live: ohw148's
            // OH68 sweep saw task pointers there) and BOTH passes then
            // "completed" without ever scanning the readers.
            SQW_TRUNC.fetch_add(1, Relaxed);
        }
    });
    if root == 0 {
        return;
    }
    SQW_ROOT.store(root, Relaxed);
    // publish the region table (idempotent per (tgid,word))
    if SQW_RN.load(Relaxed) == 0 {
        for (i, &(s, e)) in regions.iter().enumerate() {
            SQW_RSTARTS[i].store(s, Relaxed);
            SQW_RENDS[i].store(e, Relaxed);
        }
        SQW_RN.store(regions.len() as u32, Relaxed);
        if SQW_TRUNC.load(Relaxed) != 0 {
            use core::fmt::Write as _;
            let mut tw = crate::debug::types::ConsoleWriter::new();
            let _ = writeln!(
                tw,
                "SQW-TRUNC tgid={} regions hit the 512 cap — tail VMAs dropped",
                tgid
            );
        }
    }
    let rn = SQW_RN.load(Relaxed) as usize;
    let regions: alloc::vec::Vec<(u64, u64)> = if rn > 0 {
        (0..rn)
            .map(|i| (SQW_RSTARTS[i].load(Relaxed), SQW_RENDS[i].load(Relaxed)))
            .collect()
    } else {
        alloc::vec::Vec::new()
    };
    let total_mapped: u64 =
        regions.iter().map(|&(s, e)| e.saturating_sub(s)).sum();
    let word_off = |w: u64| -> Option<(usize, u64)> {
        for (i, &(s, e, f, _)) in exec_vmas.iter().enumerate() {
            if w >= s && w < e {
                return Some((i, w - s + f));
            }
        }
        None
    };
    let rd = |va: u64| -> Option<u64> { rd_word(root, va) };
    let rd32 = |va: u64| -> Option<u32> {
        rd(va & !7).map(|w| (w >> ((va & 4) * 8)) as u32)
    };
    let mut w = crate::debug::types::ConsoleWriter::new();

    // ---- 83rd: RAW phase — after the shape pass completes, scan the
    // SAME window for the lock word VALUE (±0x400 owner neighborhood).
    // Every hit prints its address and decoded neighborhood: a hit
    // inside a coroutine stack buffer names the critical-section site
    // by the saved rip sitting next to it; a hit nowhere = leaked
    // reader count. Runs on the same 1 Hz resume clock.
    if SQW_DONE.load(Relaxed) != 0 {
        if SQW_RAWDONE.load(Relaxed) != 0 {
            return;
        }
        let mut rcur = SQW_RAWCUR.load(Relaxed).max(SQW_LO);
        let hits0 = SQW_RAWHITS.load(Relaxed);
        let mut scanned = 0u64;
        let _ = writeln!(
            w,
            "SQW-RAWSCAN tgid={} lockw={:#x} cur={:#x} hits={}",
            tgid, word, rcur, hits0
        );
        'raw: for &(rs, re) in regions.iter() {
            if re <= rcur {
                continue;
            }
            rcur = rcur.max(rs);
            while rcur + 8 <= re && scanned < SQW_BUDGET {
                scanned += 1;
                if let Some(x) = rd(rcur) {
                    let near = x >= word.saturating_sub(0x400)
                        && x < word + 0x20
                        && (x & 3) == (word & 3);
                    if near && hits0 < 24 {
                        SQW_RAWHITS.fetch_add(1, Relaxed);
                        // 83rd: name the TASK that owns this hit's stack
                        // region — the thread whose pt_regs rsp lies in
                        // the same stack VMA is the holder; its park
                        // word names what it waits on (the CoWait host).
                        // Once per 8 hits (the same stack repeats).
                        if SQW_RAWHITS.load(Relaxed) % 8 == 1 {
                            let hit = rcur;
                            crate::sched::hotpath::for_each_task_try(|t| {
                                // SAFETY: diagnostic read, pt_regs only.
                                let tk = crate::sched::r#unsafe::task_shared(t);
                                if tk.tgid() as u32 != tgid {
                                    return;
                                }
                                let Some(top) = tk.kernel_stack_top_checked()
                                else {
                                    return;
                                };
                                #[cfg(feature = "x86_64")]
                                let rsp = {
                                    let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
                                    regs.rsp
                                };
                                #[cfg(not(feature = "x86_64"))]
                                let rsp = 0u64;
                                // same 1 MB granule as the hit = same
                                // stack region (thread stacks are 2-8 MB)
                                if rsp != 0
                                    && (rsp >> 20) == (hit >> 20)
                                    && rsp <= hit + 0x1000
                                {
                                    #[cfg(feature = "x86_64")]
                                    let (sys, a0) = {
                                        let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
                                        (regs.orig_rax, regs.rdi)
                                    };
                                    #[cfg(not(feature = "x86_64"))]
                                    let (sys, a0) = (0u64, 0u64);
                                    use core::fmt::Write as _;
                                    let _ = writeln!(
                                        w,
                                        "SQW-STKHIT hit={:#x} owner tid={} comm='{}' sys={} park_a0={:#x} rsp={:#x}",
                                        hit, tk.pid(), tk.comm_str(), sys, a0, rsp
                                    );
                                }
                            });
                        }
                        let _ = writeln!(
                            w,
                            "SQW-RAW hit at {:#x} val={:#x} d{:+} ctx[-0x40,+0x40]:",
                            rcur,
                            x,
                            x as i64 - word as i64
                        );
                        for i in 0..16u64 {
                            let va = rcur + i * 8 - 0x40;
                            if va & 0xfff > 0xff8 {
                                continue;
                            }
                            if let Some(y) = rd(va) {
                                let cls = match word_off(y) {
                                    Some((m, o)) => {
                                        alloc::format!("v{}+{:#x}", m, o)
                                    }
                                    None => {
                                        // printable-ASCII pair detection
                                        let b0 = (y & 0xff) as u8;
                                        let b7 = (y >> 56) as u8;
                                        if (0x20..0x7f).contains(&b0)
                                            && (0x20..0x7f).contains(&b7)
                                        {
                                            alloc::format!("{:#x} \"{}\"", y, {
                                                let bytes = y.to_le_bytes();
                                                let s: alloc::vec::Vec<u8> =
                                                    bytes.iter().copied().take_while(|&c| (0x20..0x7f).contains(&c)).collect();
                                                alloc::string::String::from_utf8_lossy(&s).into_owned()
                                            })
                                        } else {
                                            alloc::format!("{:#x}", y)
                                        }
                                    }
                                };
                                let _ = writeln!(
                                    w,
                                    "  {:#x}{} = {}",
                                    va,
                                    if va == rcur { " *HIT*" } else { "" },
                                    cls
                                );
                            }
                        }
                    }
                }
                rcur += 8;
            }
            if scanned >= SQW_BUDGET {
                break 'raw;
            }
        }
        if scanned < SQW_BUDGET {
            SQW_RAWDONE.store(1, Relaxed);
            let _ = writeln!(
                w,
                "SQW-RAWDONE tgid={} word={:#x} totalhits={} mapped={:#x}",
                tgid,
                word,
                SQW_RAWHITS.load(Relaxed),
                total_mapped
            );
        } else {
            SQW_RAWCUR.store(rcur, Relaxed);
        }
        return;
    }
    let cur = SQW_CURSOR.load(Relaxed);
    // 82nd b: lock CONTEXT dump, once per word — the words around the
    // rwlock identify the OWNING structure (a vtable pointer names the
    // class; a libc++ map header, counters etc. are readable offline).
    {
        static LOCKW_DUMPED: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        if LOCKW_DUMPED.load(Relaxed) != word {
            LOCKW_DUMPED.store(word, Relaxed);
            // module table for offline symbolization of the frames the
            // task prints carry (vN+fileoff): the OH68 line prints one
            // only for ITS park tgid — sqwalk often runs for another.
            for (i, &(s, e, f, ref n)) in exec_vmas.iter().enumerate() {
                let _ = writeln!(w, "SQW-VMA v{}={:#x}-{:#x}/{:#x}/{}", i, s, e, f, n);
            }
            let base = word & !7;
            let _ = writeln!(w, "SQW-LOCKW {:#x} ctx[-0x100,+0x40]:", word);
            for i in 0..0x28u64 {
                let va = base + i * 8 - 0x100;
                if let Some(x) = rd(va) {
                    let cls = match word_off(x) {
                        Some((m, o)) => alloc::format!("v{}+{:#x}", m, o),
                        None => alloc::format!("{:#x}", x),
                    };
                    let _ = writeln!(
                        w,
                        "  {:#x}{} = {}",
                        va,
                        if va == (word & !7) { " *LOCK*" } else { "" },
                        cls
                    );
                }
            }
        }
    }
    {
        // progress line: cursor + the covering region's extent
        let mut prog = cur;
        for &(rs, re) in regions.iter() {
            if cur >= rs && cur < re {
                prog = re;
                break;
            }
        }
        let _ = writeln!(
            w,
            "SQW-SCAN tgid={} lockw={:#x} cur={:#x}..{:#x} regions={} mapped={:#x}",
            tgid,
            word,
            cur,
            prog,
            regions.len(),
            total_mapped
        );
    }

    // ---- decode helpers ----
    let label_of = |base: u64| -> alloc::string::String {
        // libc++ string at base: SSO (len byte0>>1, chars +1..) or long
        // (cap byte0, size@+8, ptr@+16).
        let w0 = rd(base).unwrap_or(0);
        let mut lbl: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        if w0 & 1 == 0 {
            let n = ((w0 & 0xff) >> 1) as usize;
            if n <= 24 && n > 0 {
                let buf = [w0, rd(base + 8).unwrap_or(0), rd(base + 16).unwrap_or(0)];
                for i in 1..=n {
                    let b = ((buf[i / 8] >> (8 * (i % 8))) & 0xff) as u8;
                    if !(0x20..0x7f).contains(&b) {
                        return "?".into();
                    }
                    lbl.push(b);
                }
            }
        } else {
            let size = rd(base + 8).unwrap_or(0);
            let ptr = rd(base + 16).unwrap_or(0);
            if size <= 96 && sqw_canon(ptr) {
                for i in 0..size {
                    let b = rd(ptr + i).map(|x| (x & 0xff) as u8).unwrap_or(0);
                    if !(0x20..0x7f).contains(&b) {
                        break;
                    }
                    lbl.push(b);
                }
            }
        }
        alloc::string::String::from_utf8(lbl).unwrap_or_else(|_| "?".into())
    };

    // Decode one task: returns the printed line; `lockw` match appends
    // the READER marker with the saved-stack offsets that carry the
    // lock pointer (the critical-section evidence).
    let print_task = |tp: u64, tag: &str, w: &mut crate::debug::types::ConsoleWriter| {
        if !sqw_canon(tp) {
            return;
        }
        let st = (rd(tp + 48).unwrap_or(0) >> 8) & 0xff; // curStatus@49
        let lbl = label_of(tp + 216);
        let co = rd(tp + 0x130).unwrap_or(0);
        let handler = rd(tp + 440).unwrap_or(0);
        // queue name via handler->queue_ (validated by the caller or
        // probed here for referenced tasks)
        let mut qname: alloc::string::String = "?".into();
        let mut qid = 0xffff_ffffu32;
        if sqw_canon(handler) {
            if let Some(q) = rd(handler + 8) {
                if sqw_canon(q) {
                    qname = label_of(q + 200);
                    qid = rd32(q + 72).unwrap_or(0xffff_ffff);
                }
            }
        }
        let _ = write!(
            w,
            " SQW-T{}[tp={:#x} g{} st={} ft={} ct={} q=\"{}\"({}) lbl=\"{}\" wue={:#x}",
            tag,
            tp,
            rd(tp + 32).unwrap_or(0),
            st,
            rd32(tp + 112).unwrap_or(0xffff),
            rd(tp + 96).unwrap_or(0),
            qname,
            qid,
            lbl,
            rd(tp + 0x128).unwrap_or(0)
        );
        if sqw_canon(co) {
            let rsp = rd(co + 72).unwrap_or(0);
            let rip = rd(co + 80).unwrap_or(0);
            let ripoff = match word_off(rip) {
                Some((m, o)) => alloc::format!("v{}+{:#x}", m, o),
                None => "?".into(),
            };
            let stk_sz = rd(co + 0x68).unwrap_or(0).min(0x10_0000);
            let _ = write!(
                w,
                " co(st={} rip={} rsp={:#x} sz={:#x})",
                rd32(co).unwrap_or(0xffff),
                ripoff,
                rsp,
                stk_sz
            );
            // saved-stack walk: lock-word matches (READER evidence) +
            // exec-VMA frames (the critical-section site). Window: rsp
            // up to min(stack top, rsp+32KB) — the active region; the
            // buffer can be 1MB and a near-top rsp must still walk.
            // MATCH SET: the exact word AND any pointer into the lock's
            // 1KB neighborhood (ohw144 lesson: no task carried the
            // exact word — a reader inside its critical section usually
            // holds the OWNER object pointer, and the rwlock is a
            // member at owner+off; deltas are printed so the member
            // offset is readable offline).
            if rsp >= co + 120 && rsp < co + 120 + stk_sz && (rsp & 7) == 0 {
                let top = (rsp + 0x8000).min(co + 120 + stk_sz);
                let mut frames = 0;
                let mut hits = 0;
                let _ = write!(w, " lock[");
                let mut va = rsp;
                while va < top && (hits < 3 || frames < 14) {
                    if let Some(x) = rd(va) {
                        let near = x >= word.saturating_sub(0x400)
                            && x < word + 0x20
                            && (x & 3) == (word & 3);
                        if hits < 3 && near {
                            let _ = write!(
                                w,
                                "{}+{:#x}(d{:+})",
                                if hits == 0 { "" } else { "," },
                                va - rsp,
                                x as i64 - word as i64
                            );
                            hits += 1;
                        } else if frames < 14 {
                            if let Some((m, o)) = word_off(x) {
                                if frames > 0 || hits > 0 {
                                    let _ = write!(w, " ");
                                }
                                let _ = write!(w, "f:v{}+{:#x}", m, o);
                                frames += 1;
                            }
                        }
                    }
                    va += 8;
                }
                let _ = write!(w, "]");
                if hits > 0 {
                    let _ = write!(w, " READER");
                }
            }
        }
        let _ = writeln!(w);
    };

    // Queue summary: whenMap size + head entries, headTaskVec batch,
    // handler curTaskVec (dedup of task prints is by explicit lists).
    let print_queue = |q: u64, w: &mut crate::debug::types::ConsoleWriter| {
        let qname = label_of(q + 200);
        let wm_sz = rd(q + 96).unwrap_or(0);
        let _ = write!(
            w,
            " SQW-Q[q={:#x} id={} name=\"{}\" active={} exit={} wm={}]",
            q,
            rd32(q + 72).unwrap_or(0xffff),
            qname,
            rd(q + 78).unwrap_or(0) & 0xff,
            rd(q + 77).unwrap_or(0) & 0xff,
            wm_sz
        );
        // whenMap head nodes (leftmost chain, up to 6): key + task
        let mut cur = rd(q + 80).unwrap_or(0); // begin_node
        let mut n = 0;
        let _ = write!(w, " wm[");
        while sqw_canon(cur) && n < 6 {
            let key = rd(cur + 32).unwrap_or(0);
            let taskp = rd(cur + 40).unwrap_or(0);
            let _ = write!(w, "{}{:#x}:t{:#x}", if n == 0 { "" } else { "," }, key, taskp);
            // successor: right subtree's leftmost, else climb parents
            let r = rd(cur + 8).unwrap_or(0);
            if sqw_canon(r) {
                let mut x = r;
                while sqw_canon(rd(x).unwrap_or(0)) {
                    x = rd(x).unwrap_or(0);
                }
                cur = x;
            } else {
                let mut x = cur;
                loop {
                    let p = rd(x + 16).unwrap_or(0);
                    if !sqw_canon(p) {
                        x = 0;
                        break;
                    }
                    if rd(p).unwrap_or(0) != x {
                        x = p;
                        break;
                    }
                    x = p;
                }
                cur = x;
            }
            n += 1;
        }
        let _ = write!(w, "]");
        // headTaskVec batch
        let b = rd(q + 104).unwrap_or(0);
        let e = rd(q + 112).unwrap_or(0);
        if sqw_canon(b) && e >= b && (e - b) / 8 <= 16 {
            let _ = write!(w, " htv[");
            for i in 0..((e - b) / 8) {
                let _ = write!(w, "{}{:#x}", if i == 0 { "" } else { "," }, rd(b + i * 8).unwrap_or(0));
            }
            let _ = write!(w, "]");
        }
        let _ = writeln!(w);
    };

    // ---- the slice scan ----
    let mut printed = 0u32;
    let mut printed_c = 0u32;
    let mut scanned = 0u64;
    let mut cur = cur;
    'regions: for &(rs, re) in regions.iter() {
        if re <= cur {
            continue; // region already covered by earlier bursts
        }
        cur = cur.max(rs);
        while cur + 8 <= re && scanned < SQW_BUDGET {
            scanned += 1;
            // cheap prefilter: gid@32 small (both classes)
            let _gid = match rd(cur + 32) {
                Some(g) if (1..0x4_0000).contains(&g) => g,
                _ => {
                    cur += 8;
                    continue;
                }
            };
            let ty = match rd32(cur + 24) {
                Some(t) => t,
                None => {
                    cur += 8;
                    continue;
                }
            };
            let stw = rd(cur + 48).unwrap_or(0);
            let st = (stw >> 8) & 0xff;
            if ty == 3 {
                // ---- class Q: QueueTask (handler->queue_ chain) ----
                if st > 11 {
                    cur += 8;
                    continue;
                }
                let tail = rd(cur + 480).unwrap_or(0xffff_ffff_ffff_ffff);
                // isFinished@480 / onWait_@481 / isWeStart_@482 {0,1}
                if tail & 0x00ff_ffff > 0x0101_01 {
                    cur += 8;
                    continue;
                }
                // prio_@484: ffrt_queue_priority_{immediate..idle} 0..=4
                if rd32(cur + 484).unwrap_or(0xffff) > 4 {
                    cur += 8;
                    continue;
                }
                let handler = rd(cur + 440).unwrap_or(0);
                if !sqw_canon(handler) || (handler & 7) != 0 {
                    cur += 8;
                    continue;
                }
                let queue = match rd(handler + 8) {
                    Some(q) if sqw_canon(q) => q,
                    _ => {
                        cur += 8;
                        continue;
                    }
                };
                // BaseQueue validation: dequeFunc_ is libffrt code;
                // queueId_ small; printable name.
                let dq = rd(queue + 128).unwrap_or(0);
                if word_off(dq).is_none() {
                    cur += 8;
                    continue;
                }
                if rd32(queue + 72).unwrap_or(0xffff) > 0x1000 {
                    cur += 8;
                    continue;
                }
                let name_w0 = rd(queue + 200).unwrap_or(0);
                let name_ok = if name_w0 & 1 == 0 {
                    let n = ((name_w0 & 0xff) >> 1) as usize;
                    let c0 = (name_w0 >> 8) & 0xff;
                    n == 0 || c0 == b'_' as u64 || c0 == 0 || (0x41..=0x7a).contains(&c0)
                } else {
                    true // long form: decoded later
                };
                if !name_ok {
                    cur += 8;
                    continue;
                }
                if printed < 24 && (6..=8).contains(&st) {
                    // live/blocked only: FINISH-state (st=9) freed
                    // chunks passed the shape test on ohw144 and burned
                    // the whole print budget
                    print_task(cur, "", &mut w);
                    print_queue(queue, &mut w);
                }
                printed += 1;
            } else if ty <= 2 && (6..=8).contains(&st) && printed_c < 48 {
                // ---- class C: non-queue CoTask (ffrt_submit/io/uv)
                // that is EXECUTING or blocked — the read-holders may
                // be plain coroutines, not queue tasks. Structural:
                // coRoutine with a saved rsp inside its own stack.
                let co = match rd(cur + 0x130) {
                    Some(c) if sqw_canon(c) => c,
                    _ => {
                        cur += 8;
                        continue;
                    }
                };
                let rsp = match rd(co + 72) {
                    Some(r) if sqw_canon(r) => r,
                    _ => {
                        cur += 8;
                        continue;
                    }
                };
                let sz = rd(co + 0x68).unwrap_or(0);
                if sz == 0 || sz > 0x10_0000 || rsp < co + 120 || rsp >= co + 120 + sz {
                    cur += 8;
                    continue;
                }
                print_task(cur, "C", &mut w);
                printed_c += 1;
            }
            cur += 8;
        }
        if scanned >= SQW_BUDGET {
            break 'regions;
        }
    }
    if scanned < SQW_BUDGET {
        // every region covered within the budget: the walk is complete
        SQW_DONE.store(1, Relaxed);
        let _ = writeln!(
            w,
            "SQW-DONE tgid={} qtasks={} ctasks={} regions={} mapped={:#x}",
            tgid,
            printed,
            printed_c,
            regions.len(),
            total_mapped
        );
    } else {
        SQW_CURSOR.store(cur, Relaxed);
    }
}

/// 83rd: self-driving pass resume — advance the OPEN sqwalk pass from
/// the censusd kthread (250 ms rounds; the slice self-gates to one
/// burst per 500 ms). The 82nd's trigger sites (census park rows,
/// freeze_scan clusters) only OPEN passes now (sqwalk_step = arm);
/// the pass must run to completion on its own clock, and NEVER in the
/// timer softirq (ohw151's both-CPU deadlock).
pub fn sqwalk_resume() {
    sqwalk_slice();
}

/// True once the heap map COMPLETED (successfully or having burned its
/// one successful-signature pass) — the consumer stops re-arming.
pub fn ffrt_mutex_heap_map_done() -> bool {
    HEAPMAP_DONE.load(core::sync::atomic::Ordering::Relaxed) != 0
}

/// 61st: retry path — clear the burn gates so the next kthread rounds
/// re-run the (sliced) scan against the deeper-wedged heap.
pub fn ffrt_mutex_heap_map_rearm() {
    HEAPMAP_DONE.store(0, core::sync::atomic::Ordering::Relaxed);
    HM_BURNED.store(0, core::sync::atomic::Ordering::Relaxed);
    HM_CUR.store(0, core::sync::atomic::Ordering::Relaxed);
}

static HEAPMAP_DONE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

static HM_BURNED: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

pub(crate) static HM_CUR: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

static HM_FOUND: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

pub fn ffrt_mutex_heap_map(tgid: u32, known_lock: u64) -> alloc::string::String {
    use core::fmt::Write as _;
    use core::sync::atomic::Ordering::Relaxed;
    if HM_BURNED.load(Relaxed) > 0 {
        return alloc::string::String::new();
    }
    if known_lock < 0x10000 {
        return alloc::string::String::new();
    }
    // Signature: the vtable VA of the known lock (same per boot for
    // every mutexPrivate of that .so).
    let mut sig: u64 = 0;
    let mut root: u64 = 0;
    crate::sched::hotpath::for_each_task_try(|task| {
        // SAFETY: diagnostic read under the for_each_task contract.
        let tk = crate::sched::r#unsafe::task_shared(task);
        if tk.tgid() as u32 != tgid || sig != 0 {
            return;
        }
        if let Some(aspace) = tk.address_space() {
            root = aspace.root_ppn();
            sig = rd_word(root, known_lock).unwrap_or(0);
        }
    });
    if sig == 0 || root == 0 {
        return alloc::string::String::new();
    }
    // Signature + root are good: burn the one shot only now (a failed
    // signature pass must be retryable on the next kthread round —
    // ohw21 burned it on a transient address_space failure and the map
    // never printed).
    if HM_BURNED.fetch_add(1, Relaxed) > 0 {
        return alloc::string::String::new();
    }
    HEAPMAP_DONE.store(1, Relaxed);

    // 61st: the scan is SLICED — a persistent cursor resumes across
    // kthread rounds (300k words = ~0.3 s of page-table walks per
    // round). The unsliced first cut monopolized a CPU for the whole
    // scan and stalled the timer softirq (ohw23: TIMER-SOFTIRQ-STALL
    // cpu=0 age=5000ms pid=301). VMAs above 32 MB are skipped: the
    // mutexes live in the malloc arenas, not the 1-8 MB stack/pool
    // chunks that dominate samgr's large-anon volume.
    let budget: u64 = 300_000;
    let mut out = alloc::string::String::new();
    let mut found = HM_FOUND.load(Relaxed) as usize;
    let mut scanned: u64 = 0;
    let mut finished = true;
    crate::sched::hotpath::for_each_task_try(|task| {
        // SAFETY: walker-only reads of the tgid's frozen mm.
        let tk = crate::sched::r#unsafe::task_shared(task);
        if tk.tgid() as u32 != tgid {
            return;
        }
        let Some(aspace) = tk.address_space() else {
            return;
        };
        if aspace.root_ppn() != root {
            return;
        }
        let Some(m) = aspace.vma_try_read() else {
            return;
        };
        let mut cursor = HM_CUR.load(Relaxed);
        'vmas: for vma in m.iter() {
            let vt = vma.vma_type();
            // The heap class: anonymous arenas AND ashmem-backed
            // (FileBacked writable) — OH's malloc arenas may be
            // ashmem-typed; the vtable-signature match itself rejects
            // everything but mutexPrivate objects, so accept both.
            if !matches!(vt, crate::mm::types::VmaType::Anonymous)
                && !matches!(vt, crate::mm::types::VmaType::FileBacked)
            {
                continue;
            }
            if vma.flags().is_executable() {
                continue;
            }
            let start = vma.start().as_usize() as u64;
            let end = vma.end().as_usize() as u64;
            if end - start < 0x1_0000 || end - start > 0x200_0000 {
                continue;
            }
            // The FFRT mutexes all live in the SAME malloc arena as the
            // known lock (the deterministic 0x7ff0000xxxxx-class heap);
            // the 8 MB thread/coroutine stacks and the big pools sit
            // FAR above (0x7ff08xxxxxxxx) and only burn scan budget.
            // Window: ±64 MB around the known lock.
            let wlo = known_lock.saturating_sub(0x400_0000);
            let whi = known_lock.saturating_add(0x400_0000);
            if end <= wlo || start >= whi {
                continue;
            }
            if end <= cursor {
                continue;
            }
            let mut va = start.max(cursor & !0x7);
            while va < end {
                if scanned >= budget {
                    HM_CUR.store(va, Relaxed);
                    finished = false;
                    break 'vmas;
                }
                if let Some(w) = rd_word(root, va) {
                    if w == sig && va != known_lock {
                        let l = rd_word(root, va + 0x8).unwrap_or(0) & 0xffff_ffff;
                        let n = rd_word(root, va + 0x18).unwrap_or(0);
                        let mut hops = 0usize;
                        let mut ring = alloc::string::String::new();
                        let mut cur = va + 0x18;
                        for _ in 0..8 {
                            let Some(nx) = rd_word(root, cur) else {
                                break;
                            };
                            if nx == va + 0x10 {
                                break;
                            }
                            if nx == 0 || (nx >> 47) != 0 || hops >= 6 {
                                break;
                            }
                            let _ = write!(ring, "{:#x},", nx);
                            cur = nx;
                            hops += 1;
                        }
                        if (l == 1 || l == 2) && hops > 0 {
                            let _ = write!(
                                out,
                                " m{found}={:#x}l{l}ring[{hops}:{}]",
                                va,
                                ring
                            );
                            found += 1;
                            HM_FOUND.store(found as u32, Relaxed);
                        }
                    }
                }
                va += 8;
                scanned += 1;
            }
        }
        if finished {
            HM_CUR.store(0, Relaxed);
        }
    });
    if found == 0 {
        return alloc::string::String::new();
    }
    out
}

pub fn ffrt_lock_holder_scan(tgid: u32, lock: u64) -> alloc::string::String {
    use core::fmt::Write as _;
    let mut out = alloc::string::String::new();
    // The list head sits INSIDE the lock object; the lock itself
    // inside its container (SystemAbilityManager &c). Threads running
    // the container's methods hold `this` or &lock spilled — accept
    // any word in [lock-0x180, lock+0x28) and print the exact value
    // (common values across threads name the container base).
    let lo = lock.saturating_sub(0x180);
    let hi = lock + 0x28;
    crate::sched::hotpath::for_each_task_try(|task| {
        // SAFETY: diagnostic reads under the for_each_task contract.
        let tk = crate::sched::r#unsafe::task_shared(task);
        if tk.tgid() as u32 != tgid {
            return;
        }
        let Some(top) = tk.kernel_stack_top_checked() else {
            return;
        };
        #[cfg(feature = "x86_64")]
        let usp = crate::sched::r#unsafe::pt_regs_at_top(top).rsp as u64;
        #[cfg(feature = "riscv64")]
        let usp = crate::sched::r#unsafe::pt_regs_at_top(top).sp as u64;
        #[cfg(feature = "aarch64")]
        let usp = crate::sched::r#unsafe::pt_regs_at_top(top).sp as u64;
        if usp == 0 {
            return;
        }
        let Some(aspace) = tk.address_space() else {
            return;
        };
        let root = aspace.root_ppn();
        for wi in 0..2048u64 {
            // SAFETY: walker read of the parked task's frozen user
            // stack page.
            let Some(w) = (|| {
                match rux::sync::r#unsafe::pt_walk(root, usp + 8 * wi) {
                    Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
                        let kv = crate::arch::mm::memory_layout::phys_to_virt(
                            crate::arch::mm::memory_layout::PhysAddr::new(
                                (ppn << 12) | ((usp + 8 * wi) & 0xfff),
                            ),
                        );
                        // SAFETY: linear-mapped RAM word.
                        Some(rux::sync::r#unsafe::read_volatile_u64(kv.as_usize()))
                    }
                    _ => None,
                }
            })() else {
                continue;
            };
            if w >= lo && w < hi {
                let _ = write!(out, " tid{}+{:#x}(={:#x})", tk.pid(), 8 * wi, w);
                // sc57: dump the hitting thread's frame candidates —
                // the holder's park site symbolizes directly. Bounded
                // to the first 6 hitting tids.
                {
                    // per-TID budget (a waiter thread hits the range at
                    // several spill slots; without this the first
                    // thread's repeats burn the whole budget)
                    static HOLDER_FRAMED_TIDS: [core::sync::atomic::AtomicU32; 8] =
                        [const { core::sync::atomic::AtomicU32::new(0) }; 8];
                    let tno = tk.pid();
                    if HOLDER_FRAMED_TIDS.iter().any(|t| {
                        t.load(core::sync::atomic::Ordering::Relaxed) == tno as u32
                    }) {
                        return; // already framed
                    } else {
                        let mut claimed = false;
                        for t in HOLDER_FRAMED_TIDS.iter() {
                            if t.compare_exchange(
                                0,
                                tno as u32,
                                core::sync::atomic::Ordering::Relaxed,
                                core::sync::atomic::Ordering::Relaxed,
                            )
                            .is_ok()
                            {
                                claimed = true;
                                break;
                            }
                        }
                        if !claimed {
                            return;
                        }
                    }
                    let Some(aspace2) = tk.address_space() else {
                        return;
                    };
                    let root2 = aspace2.root_ppn();
                    let Some(_mgr2) = aspace2.vma_try_read() else {
                        return;
                    };
                    let mut fr = alloc::string::String::new();
                    for k in 0..96u64 {
                        let fa = usp + 8 * k;
                        let Some(x) = (|| {
                            match rux::sync::r#unsafe::pt_walk(root2, fa) {
                                Some((ppn2, bits2))
                                    if bits2 & 1 != 0 && bits2 & 0x4 != 0 && ppn2 != 0 =>
                                {
                                    let kv2 = crate::arch::mm::memory_layout::phys_to_virt(
                                        crate::arch::mm::memory_layout::PhysAddr::new(
                                            (ppn2 << 12) | (fa & 0xfff),
                                        ),
                                    );
                                    // SAFETY: linear-mapped RAM word.
                                    Some(rux::sync::r#unsafe::read_volatile_u64(kv2.as_usize()))
                                }
                                _ => None,
                            }
                        })() else {
                            continue;
                        };
                        if matches!(
                            _mgr2.find(crate::mm::types::VirtAddr::new(x as usize)),
                            Some(v2) if v2.flags().is_executable()
                        ) {
                            let _ = write!(fr, " {:#x}", x);
                        }
                    }
                    let _ = write!(out, " tid{}fr:{}", tk.pid(), fr);
                }
            }
        }
    });
    out
}

static RWFRZ_MM: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

static RWFRZ_TGID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

static RWFRZ_FIRST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

static RWFRZ_LASTVAL: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

static RWFRZ_LASTVAL_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

static RWFRZ_DUMPED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// sc82: per-word inflation-curve table. Each contended rwlock-shaped
/// word with readers >= 2 is tracked (last value + last print); one
/// line per word per minute carries the delta — a gradual +1 pile-up
/// (real readers parking in scopes) vs a single jump (count corruption
/// or a fork-inherited copy) is decided by this curve, not by the
/// terminal value alone.
struct HighWord {
    uaddr: usize,
    tgid: u32,
    lastv: u32,
    at: u64,
}

/// huks-wedge lane (hw2): does this tgid own a standing frozen word in
/// the census table? The holder-exit print (exit.rs) fires for any
/// thread of such a tgid — the holder that froze the word is exactly
/// the thread whose death must be decoded.
pub fn rwfrz_tgid_frozen(tgid: u32) -> bool {
    let hl = RWFRZ_HIGHTAB.lock_irqsave();
    let (hn, htab) = &*hl;
    (0..*hn).any(|i| htab[i].tgid == tgid && htab[i].uaddr != 0)
}

static RWFRZ_HIGHTAB: Spinlock<(usize, [HighWord; 8])> = Spinlock::new((
    0,
    [const {
        HighWord {
            uaddr: 0,
            tgid: 0,
            lastv: 0,
            at: 0,
        }
    }; 8],
));

static RWFRZ_BEAT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Last tid census of the watched tgid + its size (leak forensics: a tid
/// that vanishes while the word never drains is a reader that exited).
static RWFRZ_TIDS: Spinlock<(usize, [u32; 64])> = Spinlock::new((0, [0; 64]));

/// Which tgid RWFRZ_TIDS currently censuses (sc82 early-watch support).
static RWFRZ_TIDS_CENSUS_TGID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Global thread-exit ring (sc82): every task unlink (thread OR process
/// leader) records (jiffies, pid, tgid). The freeze dump replays the
/// watched tgid's exits — readers that died BEFORE the watch adopted the
/// word (leak evidence with timestamps) instead of only post-adoption
/// vanishings. Forensics only; no behavior.
#[derive(Clone, Copy)]
struct ExitRec {
    j: u64,
    pid: u32,
    tgid: u32,
}

static EXIT_RING: Spinlock<(usize, [ExitRec; 256])> = Spinlock::new((0, [const { ExitRec { j: 0, pid: 0, tgid: 0 } }; 256]));

/// Called from pid_hash_remove under the bucket lock — keep it to two
/// atomic-ish stores (one short critical section on the ring lock).
/// sc86: when a word is adopted and the dying task belongs to the watched
/// tgid, ALSO snapshot the word through the dying task's page tables
/// (if its mm is still alive at unlink time) and print comm + value —
/// a reader that exits without the count decrementing is caught in the
/// act. The page-table read is skipped under the ring lock.
pub fn note_task_unlink(pid: u32, tgid: u32) {
    let j = crate::timer::get_jiffies() as u64;
    let mut g = EXIT_RING.lock_irqsave();
    let (ref mut head, ref mut ring) = *g;
    ring[*head % 256] = ExitRec { j, pid, tgid };
    *head += 1;
    drop(g);
    // Post-adoption exit snapshot (diagnostics only; no locks held here —
    // pid_hash_remove's bucket lock IS still held by the caller, so the
    // page walk must not take pid-hash locks; PageTableWalker does not).
    let uaddr = RWFRZ_UADDR.load(core::sync::atomic::Ordering::Relaxed);
    if uaddr != 0 && tgid == RWFRZ_TGID.load(core::sync::atomic::Ordering::Relaxed) {
        let t = crate::sched::hotpath::find_task_by_pid(pid);
        if !t.is_null() {
            let tk = crate::sched::r#unsafe::task_shared(t);
            let comm = tk.comm();
            let name = core::str::from_utf8(&comm[..])
                .unwrap_or("")
                .trim_end_matches('\0');
            let word = rux::sync::r#unsafe::read_u32_via_task(t, uaddr);
            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
            crate::pr_warn!(
                "RWFRZ: EXIT-WORD tid {} comm='{}' word={:#x}",
                pid,
                name,
                word.unwrap_or(0xdeadbeef)
            );
        }
    }
}

fn exit_ring_dump_tgid(tgid: u32) {
    use core::fmt::Write;
    let mut s = alloc::string::String::new();
    let g = EXIT_RING.lock_irqsave();
    let head = g.0;
    let start = head.saturating_sub(256);
    for i in start..head {
        let r = g.1[i % 256];
        if r.tgid == tgid && r.pid != 0 {
            let _ = write!(s, " [pid {} @{}s]", r.pid, r.j / 100);
        }
    }
    drop(g);
    if s.is_empty() {
        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
        crate::pr_warn!("RWFRZ: exit ring has NO exits for tgid {} — readers did not die by task unlink", tgid);
    } else {
        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
        crate::pr_warn!("RWFRZ: exit ring for tgid {}:{}", tgid, s);
    }
}

/// sc93 launcher gate: AMS SIGKILLs a lifecycle-timed-out app while the
/// DEBUG userspace dumper itself fails on that app (every LIFECYCLE_TIMEOUT
/// freeze artifact says "normal stack:failed to fully dump due to
/// timeout") — the kernel is the only observer that can still read the
/// wedged process. Called at SIGKILL send time (signal.rs), when the app
/// is by definition terminally wedged: prints every thread of the tgid,
/// each thread's futex park word + its CURRENT value (binder reply waits,
/// condvars, mutexes — the park value names the primitive and protocol),
/// and rip/rsp + file-backed stack frames via the sc90 exec-VMA filter.
/// pr_warn so the quiet boot (loglevel=4) keeps it; 8 dumps/boot cap
/// against restart-loop flooding.
pub fn kill_dump_tgid(tgid: u32) {
    use core::fmt::Write;
    static N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
    if N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) >= 8 {
        return;
    }
    // Collect thread pointers only under the pid-hash walk (its bucket
    // locks forbid heavy work); the page-table reads happen after.
    // for_each_task takes an Fn closure, so the list goes through a
    // RefCell.
    // sc244 (leg-17): PIN every collected task. The dump runs at
    // SIGKILL send time, so the victim's threads exit CONCURRENTLY —
    // an unpinned raw pointer freed mid-dump printed a poisoned tid
    // (0xdeadbeef) and page-faulted the boot at the launcher floor
    // (sc244, t=1466, pidmax 4834). try_pin under the bucket lock is
    // the documented pin discipline; task_put at the end.
    let threads_cell = core::cell::RefCell::new(alloc::vec::Vec::<*mut Task>::new());
    crate::sched::hotpath::for_each_task(|t| {
        if t.tgid() as u32 == tgid {
            let t = t as *const Task as *mut Task;
            if crate::process::types::Task::try_pin(t) {
                threads_cell.borrow_mut().push(t);
            }
        }
    });
    let threads = threads_cell.into_inner();
    let mut hdr = alloc::string::String::new();
    let _ = write!(hdr, "KILLDUMP: tgid {} threads={}", tgid, threads.len());
    for &t in threads.iter() {
        let tk = crate::sched::r#unsafe::task_shared(t);
        let comm = core::str::from_utf8(tk.comm()).unwrap_or("").trim_end_matches('\0');
        let _ = write!(hdr, " [{}:{}]", tk.pid(), comm);
    }
    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
    crate::pr_warn!("{}", hdr);
    for &t in threads.iter() {
        // Park word: this task's live (unwoken) entry in the waiter pool.
        let mut parked = None;
        for slot in WAITER_POOL.iter() {
            let s = slot.lock_irqsave();
            if let Some(w) = s.as_ref() {
                if core::ptr::eq(w.task, t) && !w.woken {
                    parked = Some((w.key.uaddr, w.bitset));
                }
            }
            drop(s);
            if parked.is_some() {
                break;
            }
        }
        let tk = crate::sched::r#unsafe::task_shared(t);
        let comm = core::str::from_utf8(tk.comm()).unwrap_or("").trim_end_matches('\0');
        match parked {
            Some((uaddr, bitset)) => {
                let word = rux::sync::r#unsafe::read_u32_via_task(t, uaddr);
                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                crate::pr_warn!(
                    "KILLDUMP: tid {} comm='{}' parked uaddr={:#x} bitset={:#x} word={:#x}",
                    tk.pid(),
                    comm,
                    uaddr,
                    bitset,
                    word.unwrap_or(0xdead_bee1)
                );
            }
            None => {
                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                crate::pr_warn!(
                    "KILLDUMP: tid {} comm='{}' NOT on a futex (blocked in another syscall or runnable)",
                    tk.pid(),
                    comm
                );
            }
        }
        // sc93: the saved entry frame carries the blocked syscall itself
        // — orig_rax (syscall nr) + args. For a task parked mid-syscall
        // this names the wait primitive outright (read/epoll/ioctl and
        // WHICH fd). Fd-like args (< 64) are resolved to paths.
        //
        // Stack-freed guard (KILLDUMP census race, monkey-b r38 +
        // monkey-a r32, identical badaddr 0x13ff0 on v9-x86 and v10-x86):
        // the pinned Task can outlive its own exit's free_kernel_stack()
        // by an unbounded window (reaping is asynchronous), and a
        // stackless task's kernel_stack_top() == KERNEL_STACK_SIZE — the
        // pt_regs read below then faults INSIDE the printer and panics
        // the boot. Skip the whole per-thread dump (entry frame, wedge
        // dump, frame walk) for such tasks; the header line already
        // counted them, so say why their lines are missing.
        let Some(_top) = tk.kernel_stack_top_checked() else {
            // HOTPATH-PRINT: census/forensic emitter (1 Hz heartbeat- or wedge-gated caller; cold path).
            crate::pr_warn!(
                "KILLDUMP: tid {} stack-freed (exited mid-census; entry-frame/wedge/frame-walk skipped)",
                tk.pid()
            );
            continue;
        };
        {
            let top = _top;
            #[cfg(feature = "x86_64")]
            {
                let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
                // SAFETY: diagnostic read of this task's entry frame.
                let r = regs;
                // HOTPATH-PRINT: census/forensic emitter (1 Hz heartbeat- or wedge-gated caller; cold path).
                crate::pr_warn!(
                    "KILLDUMP: tid {} syscall nr={:#x} rdi={:#x} rsi={:#x} rdx={:#x} r10={:#x}",
                    tk.pid(),
                    r.orig_rax,
                    r.rdi,
                    r.rsi,
                    r.rdx,
                    r.r10
                );
                let mut fdnote = alloc::string::String::new();
                for arg in [r.rdi, r.rsi, r.rdx] {
                    if arg < 64 {
                        // sc210: None-safe fdtable walk (just-forked task
                        // has no fdtable yet; fdtable() panics).
                        if let Some(f) = tk
                            .try_fdtable()
                            .and_then(|fdt| fdt.get_file(arg as usize))
                        {
                            use core::fmt::Write;
                            let _ = write!(fdnote, " fd{}={}", arg, f.path());
                        }
                    }
                }
                if !fdnote.is_empty() {
                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                    crate::pr_warn!("KILLDUMP: tid {} {}", tk.pid(), fdnote);
                }
                // sc97 wedge forensics: the sc96 kills put the launcher
                // main thread in writev(fd=3) on an inode-less file 6/8
                // times (plus one mount(2) and one open(2) park). Name
                // the fd TYPE by ops identity and dump the state that
                // decides the wedge; read the syscall's user arguments
                // (iovec layout + payload head, path/fstype strings); and
                // print the task run state so "parked" vs "runnable"
                // (slow exit path) can be told apart.
                kill_dump_wedge(tk, t, r.orig_rax, [r.rdi, r.rsi, r.rdx]);
            }
        }
        rwfz_stack_frames(t, "KILLDUMP", 10, 96, false);
    }
    // sc244: release the pins taken at collection (may free the last
    // references of the concurrently-dying threads — schedulable
    // context, no locks held).
    for &t in threads.iter() {
        crate::process::types::Task::task_put(t);
    }
}

/// sc97: type name for a File by fops identity (the kernel-image symbol
/// at the ops address names any not listed here, offline via nm).
fn kd_fd_type(file: &crate::fs::types::File) -> &'static str {
    let ops_ref = match file.get_ops() {
        Some(o) => o,
        None => return "no-ops",
    };
    let ops = ops_ref as *const crate::fs::types::FileOps as *const ();
    let is = |p: &crate::fs::types::FileOps| ops == (p as *const _ as *const ());
    if rux::api::fs::is_pipe_ops(ops_ref) {
        return "pipe";
    }
    if rux::api::fs::is_fifo_ops(ops_ref) {
        return "fifo";
    }
    if rux::api::net::is_unix_socket_ops(ops_ref) {
        return "unix-sock";
    }
    if rux::api::net::is_inet_socket_ops(ops_ref) {
        return "inet-sock";
    }
    if is(&crate::ipc::hotpath::BINDER_OPS) {
        return "binder";
    }
    if rux::api::fs::is_inotify_ops(ops_ref) {
        return "inotify";
    }
    if file.get_ops().is_some_and(crate::syscall::support::is_eventfd_ops) {
        return "eventfd";
    }
    "other"
}

/// sc97: deep dump of the wedged syscall's fd and user arguments.
/// `t` is the live Task whose entry frame the args came from (INV-SCE-2
/// derivation; `tk` is its shared reference).
fn kill_dump_wedge(tk: &Task, t: *mut Task, nr: u64, args: [u64; 3]) {
    use core::fmt::Write;
    // sc97 discriminator: RUNNING-state at SIGKILL time means the entry
    // frame's syscall may be STALE (task is executing userspace — e.g. a
    // busy loop) or the task burns kernel time in a non-sleeping loop, or
    // it was set runnable but never linked/scheduled (wake-loss family).
    // utime vs stime names the burn mode; tiny both = never scheduled.
    let linked = match crate::sched::hotpath::grq_diag_cfs_linked(t) {
        Some(b) => {
            if b { "1" } else { "0" }
        }
        None => "n/a",
    };
    // HOTPATH-PRINT: census/forensic emitter (1 Hz heartbeat- or wedge-gated caller; cold path).
    crate::pr_warn!(
        "KILLDUMP: tid {} state={:#x} on_cpu={} cfs_linked={} pick_state={} cpu={} utime={}t stime={}t",
        tk.pid(),
        tk.state().bits(),
        tk.on_cpu(),
        linked,
        tk.pick_state(),
        tk.ti_cpu(),
        tk.utime_ticks.load(core::sync::atomic::Ordering::Relaxed),
        tk.stime_ticks.load(core::sync::atomic::Ordering::Relaxed)
    );
    // sc105 stime-burn attribution: page-shaped positional reads (the mmap
    // demand-fault signature) with service time, cache-miss fills, and the
    // timer-tick kernel-rip histogram (per comm, 4 KiB pages). Global
    // counters — deltas across consecutive launcher kills isolate this
    // child's share; rip pages symbolize via nm/addr2line.
    {
        let (reads, misses, cycles, fills) = rux::api::ext4::demand_read_stats();
        let mhz = {
            #[cfg(feature = "x86_64")]
            {
                crate::arch::x86_64::cpu::get_counter_freq() / 1_000_000
            }
            #[cfg(not(feature = "x86_64"))]
            {
                0
            }
        };
        let us = if mhz > 0 { cycles / mhz } else { 0 };
        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
        crate::pr_warn!(
            "KILLDUMP: demand-fault reads={} misses={} fillmisses={} cyc={} (~{}us avg={}us)",
            reads,
            misses,
            fills,
            cycles,
            us,
            if reads > 0 { us / reads } else { 0 }
        );
        #[cfg(feature = "x86_64")]
        {
            let rows = crate::arch::x86_64::trap::stime_profile_top(true);
            for (comm, page, ticks) in rows.iter().take(8) {
                let comm_str = core::str::from_utf8(comm)
                    .unwrap_or("")
                    .trim_end_matches('\0');
                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                crate::pr_warn!(
                    "KILLDUMP: stime-page(FOCUS) comm='{}' rip_page={:#x} ticks={}",
                    comm_str,
                    page,
                    ticks
                );
            }
            let rows = crate::arch::x86_64::trap::stime_profile_top(false);
            for (comm, page, ticks) in rows.iter().take(8) {
                let comm_str = core::str::from_utf8(comm)
                    .unwrap_or("")
                    .trim_end_matches('\0');
                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                crate::pr_warn!(
                    "KILLDUMP: stime-page comm='{}' rip_page={:#x} ticks={}",
                    comm_str,
                    page,
                    ticks
                );
            }
        }
    }
    // Whole low fdtable: fd/type/path one line — where fd3 came from and
    // what the process holds overall.
    // sc210: try_fdtable — the wedge dump can observe a task whose
    // fdtable is not initialized (just-forked); fdtable() would panic
    // ("FdTable not initialized", task.rs) and take the whole boot down
    // with a kernel panic mid-census — exactly how sc210 (the deepest
    // boot ever, launcher executing) died at t=1410s.
    {
        let mut line = alloc::string::String::new();
        let _ = write!(line, "KILLDUMP: tid {} fdtab:", tk.pid());
        for fd in 0..16usize {
            if let Some(f) = tk.try_fdtable().and_then(|fdt| fdt.get_file(fd)) {
                let _ = write!(line, " [{}:{}:{}]", fd, kd_fd_type(&f), f.path());
            }
        }
        // HOTPATH-PRINT: census/forensic emitter (1 Hz heartbeat- or wedge-gated caller; cold path).
        crate::pr_warn!("{}", line);
    }
    // The syscall's primary fd (rdi for read/write/writev/ioctl-family).
    if let Some(&fd) = args.first() {
        if fd < 64
            && tk.try_fdtable()
                .map(|fdt| fdt.get_file(fd as usize).is_some())
                .unwrap_or(false)
        {
            if let Some(f) = tk.try_fdtable().and_then(|fdt| fdt.get_file(fd as usize)) {
                let ty = kd_fd_type(&f);
                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                crate::pr_warn!(
                    "KILLDUMP: tid {} fd{} type={} flags={:#x}",
                    tk.pid(),
                    fd,
                    ty,
                    f.flags_bits()
                );
                match ty {
                    "pipe" => {
                        if let Some((fill, cap, rc, wc)) = rux::api::fs::pipe_diag_snapshot(&f) {
                            let pipe_ptr = f.private_data_ptr()
                                .map(|p| p as usize)
                                .unwrap_or(0);
                            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                            crate::pr_warn!(
                                "KILLDUMP: tid {} pipe fill={}/{} rclosed={} wclosed={} ptr={:#x}",
                                tk.pid(),
                                fill,
                                cap,
                                rc,
                                wc,
                                pipe_ptr
                            );
                            // Who else holds an end? create_pipe installs the
                            // same Arc<Pipe> raw pointer in BOTH files, so a
                            // private_data match names every holder task and
                            // which end (O_RDONLY = reader) it owns.
                            if pipe_ptr != 0 {
                                let holders_cell =
                                    core::cell::RefCell::new(alloc::string::String::new());
                                crate::sched::hotpath::for_each_task(|ht| {
                                    let hk = crate::sched::r#unsafe::task_shared(ht);
                                    if hk.pid() == 0 {
                                        return;
                                    }
                                    for hfd in 0..32usize {
                                        // sc210: None-safe — the census walks
                                        // every task; a just-forked one has
                                        // no fdtable yet.
                                        if let Some(hf) = hk
                                            .try_fdtable()
                                            .and_then(|fdt| fdt.get_file(hfd))
                                        {
                                            let hty = kd_fd_type(&hf);
                                            if hty == "pipe"
                                                && hf.private_data_ptr()
                                                    == Some(pipe_ptr as *mut u8)
                                            {
                                                let hcomm = core::str::from_utf8(hk.comm())
                                                    .unwrap_or("")
                                                    .trim_end_matches('\0');
                                                let _ = write!(
                                                    holders_cell.borrow_mut(),
                                                    " [{}:{} fd{} {}]",
                                                    hk.pid(),
                                                    hcomm,
                                                    hfd,
                                                    if hf.flags_bits() & 0o3 == 0 {
                                                        "READ"
                                                    } else {
                                                        "WRITE"
                                                    }
                                                );
                                            }
                                        }
                                    }
                                });
                                let holders = holders_cell.into_inner();
                                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                                crate::pr_warn!(
                                    "KILLDUMP: tid {} pipe holders:{}",
                                    tk.pid(),
                                    if holders.is_empty() { " NONE" } else { &holders }
                                );
                            }
                        }
                    }
                    "unix-sock" => {
                        if let Some(us) = rux::api::net::unix_file_diag(&f) {
                            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                            crate::pr_warn!(
                                "KILLDUMP: tid {} unix kind={} bound={:?} dgram_peer={:?} queued={}B truesize={} sndbuf={} qlen={}",
                                tk.pid(),
                                us.kind,
                                us.bound,
                                us.peer,
                                us.queued_bytes,
                                us.truesize,
                                us.sndbuf,
                                us.qlen
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    // User-argument payloads: writev/write iovec heads (libhilog writes
    // header28/tag/content — the payload names the producer), open/mount
    // path strings.
    match nr {
        20 | 1 | 19 => {
            // writev / write / readv — iovec array at args[1] (writev) or
            // raw buffer for write.
            if nr == 20 {
                let mut iovs = alloc::vec![0u8; 3 * 16];
                let n = rux::sync::r#unsafe::read_bytes_via_task(t, args[1] as usize, &mut iovs);
                for i in 0..3 {
                    if n < (i + 1) * 16 {
                        break;
                    }
                    let base = u64::from_le_bytes(iovs[i * 16..i * 16 + 8].try_into().unwrap());
                    let len = u64::from_le_bytes(iovs[i * 16 + 8..i * 16 + 16].try_into().unwrap());
                    let mut head = alloc::vec![0u8; 48];
                    let hn = rux::sync::r#unsafe::read_bytes_via_task(t, base as usize, &mut head);
                    let mut txt = alloc::string::String::new();
                    for &b in &head[..hn] {
                        if b == 0 {
                            break;
                        }
                        if (0x20..0x7f).contains(&b) {
                            txt.push(b as char);
                        } else {
                            txt.push('.');
                        }
                    }
                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                    crate::pr_warn!(
                        "KILLDUMP: tid {} iov{} base={:#x} len={} head='{}'",
                        tk.pid(),
                        i,
                        base,
                        len,
                        txt
                    );
                }
            }
        }
        2 => {
            // open(2): pathname at args[0]
            let p = rux::sync::r#unsafe::read_cstr_via_task(t, args[0] as usize, 96);
            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
            crate::pr_warn!("KILLDUMP: tid {} open path='{}'", tk.pid(), p);
        }
        165 => {
            // mount(2): source/target/fstype at args[0..2]
            let s = rux::sync::r#unsafe::read_cstr_via_task(t, args[0] as usize, 96);
            let tg = rux::sync::r#unsafe::read_cstr_via_task(t, args[1] as usize, 96);
            let ft = rux::sync::r#unsafe::read_cstr_via_task(t, args[2] as usize, 32);
            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
            crate::pr_warn!(
                "KILLDUMP: tid {} mount src='{}' tgt='{}' fstype='{}'",
                tk.pid(),
                s,
                tg,
                ft
            );
        }
        _ => {}
    }
}

/// 81st: read the parked word's CURRENT value through the parked
/// task's page tables (shape test for the frozen-rwlock class).
/// INV-SY-1 in rux::sync::unsafe owns the read; kept here as the
/// pub face binder's diagnostic uses.
pub fn read_park_word_via_task(task: *mut crate::process::types::Task, word: u64) -> Option<u32> {
    if task.is_null() || word < 0x1000 {
        return None;
    }
    rux::sync::r#unsafe::read_u32_via_task(task as *const Task, word as usize)
}

// The cross-task user-memory diagnostic readers (read_bytes_via_task,
// read_cstr_via_task, read_u32_via_task) moved into rux::sync::unsafe
// (INV-SY-1).

/// 81st: reader-identity scan for a FROZEN rwlock word. The 68th-era
/// freeze dump needed waiters>=3 to adopt (our freezes carry 1-2 waiters:
/// the parked looper itself), so the frozen 16-reader words (value
/// 0x80000010-class, ohw135 tid 1583's park) never got the full
/// treatment. A reader inside its critical section keeps the lock
/// object pointer in a stack slot: scan every task of the tgid for the
/// word address in the 160 words above its entry rsp and print each
/// hit's tid/comm/entry-syscall/park-uaddr — naming the readers and
/// what EACH is blocked on. ConsoleWriter (censusd drops printk).
pub fn rwfrz_reader_scan(tgid: u32, word: u64) {
    use core::fmt::Write as _;
    #[cfg(feature = "x86_64")]
    const FUTEX_SYSNO: u64 = 202;
    #[cfg(not(feature = "x86_64"))]
    const FUTEX_SYSNO: u64 = 0;
    let mut w = crate::debug::types::ConsoleWriter::new();
    let mut hits = 0u32;
    let _ = writeln!(w, "RWFRZ-READERS tgid={} word={:#x}:", tgid, word);
    // ohw139 lesson: for_each_task runs the callback under the pid-hash
    // bucket lock — 640x2 page-table walks per task under that spinlock
    // soft-locked CPU#1 for 10 s+ and killed the console mid-print.
    // for_each_task_try is the diagnostic contract the oh68 sweeps use
    // (no bucket lock held across the walk).
    crate::sched::hotpath::for_each_task_try(|t| {
        if hits >= 24 {
            return;
        }
        // SAFETY: pid-hash walk contract; reads confined to pt-regs and
        // the task's own user pages.
        let tk = crate::sched::r#unsafe::task_shared(t);
        if tk.tgid() as u32 != tgid {
            return;
        }
        let Some(top) = tk.kernel_stack_top_checked() else {
            return;
        };
        #[cfg(feature = "x86_64")]
        let (sys, a0, rsp) = {
            let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
            { (regs.orig_rax, regs.rdi, regs.rsp) }
        };
        #[cfg(not(feature = "x86_64"))]
        let (sys, a0, rsp) = (0u64, 0u64, 0u64);
        if rsp == 0 || rsp > 0x7fff_ffff_ffff {
            return;
        }
        let mut found_at: [u64; 3] = [0; 3];
        let mut deltas: [i64; 3] = [0; 3];
        let mut nf = 0usize;
        // 81st: 640 words — the rwlock WAITERS park at the lock (pointer
        // near rsp), but readers stuck INSIDE their critical section hold
        // it deeper (nested SendRequest frames) — ohw136's 160-word cut
        // only caught the 6 samgr waiters, never the 16 holders.
        // 83rd: 640 -> 4096 words (32 KB) — ohw149's full-window heap
        // scan found the word on NO heap stack either, so the remaining
        // thread-side blind spot is depth: a reader that rdlocked and
        // then nested 10-40 KB of C++ frames into a binder call keeps
        // the unlock pointer ABOVE a 5 KB window. ~30 tasks x 4096 x 2
        // reads ~= 250k page-walks, the proven reader-scan cost class.
        // 82nd: match the lock's 1KB NEIGHBORHOOD (owner-pointer holders
        // — the rwlock is a member and readers usually hold the owner
        // object; exact-word matching alone found only waiters on
        // ohw141/143/144).
        for i in 0..4096u64 {
            if nf == 3 {
                break;
            }
            let va = rsp + 8 * i;
            if va & 0xfff > 0xff8 {
                continue; // page-cross split — skip
            }
            let lo = rux::sync::r#unsafe::read_u32_via_task(t, va as usize).unwrap_or(0) as u64
                | ((rux::sync::r#unsafe::read_u32_via_task(t, va as usize + 4).unwrap_or(0) as u64) << 32);
            let near = lo >= word.saturating_sub(0x400)
                && lo < word + 0x20
                && (lo & 3) == (word & 3);
            if near {
                found_at[nf] = va;
                deltas[nf] = lo as i64 - word as i64;
                nf += 1;
            }
        }
        if nf == 0 {
            return;
        }
        hits += 1;
        let comm = tk.comm();
        let name = core::str::from_utf8(&comm[..])
            .unwrap_or("")
            .trim_end_matches('\0');
        let _ = writeln!(
            w,
            "  RDR tid={} comm='{}' sys={} park_a0={:#x} slots={:#x},{:#x},{:#x} deltas={:+},{:+},{:+}",
            tk.pid(),
            name,
            sys,
            a0,
            found_at[0],
            found_at[1],
            found_at[2],
            deltas[0],
            deltas[1],
            deltas[2]
        );
        if sys == FUTEX_SYSNO && a0 != 0 {
            // what this reader is blocked on, plus its wait word's shape
            let w0 = rux::sync::r#unsafe::read_u32_via_task(t, a0 as usize).unwrap_or(0);
            let _ = writeln!(w, "    parkword={:#x} val={:#x}", a0 as u64, w0);
        }
    });
    let _ = writeln!(w, "RWFRZ-READERS end hits={} [118m]", hits);
    // 118th (mouse-endgame): malloc-lock holder hunt, INLINED here
    // (a standalone fn with the same body failed to reach the binary
    // while edits inside this fn land reliably). The standing word on
    // the ld-musl data page 0x3fbf13e000 is __malloc_lock[0] (bss
    // static at vaddr 0x13e6a8 of the oh image's ld-musl, LOCK() from
    // mallocng per musl glue.h; bit31 = held, low = 1+waiters). ls1:
    // foundation's copy held-forever from <=12.43 s with 28 waiters,
    // no thread parked inside malloc on any syscall, none running —
    // holder exited holding or recursive-malloc self-deadlock. Walk
    // every same-tgid thread's stack and count allocator frames:
    //   WRAP [base+0x12adb0, base+0x12b4b8) malloc/calloc/free/...
    //   NG   [base+0xba000, base+0xbd000) mallocng internals
    //   HOOK [base+0x12bb00, base+0x12c500) OH malloc-hook cluster
    // waiter with >=2 WRAP frames = recursive holder (named); thread
    // parked OFF the word with WRAP/NG frames = stuck holder (named);
    // neither = thread-side holders acquitted (holder exited).
    // x86_64-only (pcachewedge build fix, 2026-11-15): the scan reads
    // x86 pt_regs field names (orig_rax/rdi/rsp) and the OH x86_64
    // image's fixed musl load base — it cannot compile or mean anything
    // on the other arches; this block had escaped its cfg and broke
    // every non-x86_64 build of main f5ed08765.
    // 120th ROOT-CAUSE CORRECTION (see the do_futex CTD-EARLY comment):
    // the old 32-bit-mask page gate was always-false and DCE'd this
    // entire hunt out of release builds. 64-bit page mask, plain literal.
    #[cfg(target_arch = "x86_64")]
    if word & 0xffff_ffff_f000 == 0x3fbf_13e000 {
        let mut w2 = crate::debug::types::ConsoleWriter::new();
        let _ = writeln!(w2, "MHUNT-CANARY2 word={:#x}", word);
        let _ = writeln!(w2, "MHUNT tgid={} word={:#x}:", tgid, word);
        let mut rows = 0u32;
        let mut waiters = 0u32;
        let mut v_rec: Option<u64> = None;
        let mut v_stuck: Option<(u64, u64, u64)> = None;
        let musl = musl_anchor(tgid);
        crate::sched::hotpath::for_each_task_try(|t| {
            if rows >= 24 {
                return;
            }
            // SAFETY: pid-hash walk contract; reads confined to pt-regs
            // and the task's own user pages.
            let tk = crate::sched::r#unsafe::task_shared(t);
            if tk.tgid() as u32 != tgid {
                return;
            }
            let Some(top) = tk.kernel_stack_top_checked() else {
                return;
            };
            // 120th fix: the 118th's inlined copy of this hunt missed the
            // cfg gate the standalone malloc_holder_scan carries — every
            // non-x86_64 build of the tree (default `make build`) failed
            // E0433 on `crate::arch::x86_64`. Same gating as the twin.
            #[cfg(feature = "x86_64")]
            let (sys, a0, rsp) = {
                let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
                { (regs.orig_rax, regs.rdi, regs.rsp) }
            };
            #[cfg(not(feature = "x86_64"))]
            let (sys, a0, rsp) = (0u64, 0u64, 0u64);
            if rsp == 0 || rsp > 0x7fff_ffff_ffff {
                return;
            }
            let on_word = sys == FUTEX_SYSNO && a0 == word;
            if on_word {
                waiters += 1;
            }
            let mut wrap_hits: [u64; 3] = [0; 3];
            let mut ng_hits: [u64; 2] = [0; 2];
            let mut hook_hits: [u64; 2] = [0; 2];
            let mut nw = 0usize;
            let mut nn = 0usize;
            let mut nh = 0usize;
            for i in 0..4096u64 {
                let va = rsp + 8 * i;
                if va & 0xfff > 0xff8 {
                    continue; // page-cross split — skip
                }
                let lo = rux::sync::r#unsafe::read_u32_via_task(t, va as usize).unwrap_or(0) as u64
                    | ((rux::sync::r#unsafe::read_u32_via_task(t, va as usize + 4).unwrap_or(0) as u64) << 32);
                let cls = musl.map(|a| musl_off_class(a, lo)).unwrap_or(0);
                if cls == 1 {
                    if nw < 3 {
                        wrap_hits[nw] = lo;
                    }
                    nw += 1;
                } else if cls == 2 {
                    if nn < 2 {
                        ng_hits[nn] = lo;
                    }
                    nn += 1;
                } else if cls == 3 {
                    if nh < 2 {
                        hook_hits[nh] = lo;
                    }
                    nh += 1;
                }
            }
            if nw == 0 && nn == 0 {
                return;
            }
            rows += 1;
            let comm = tk.comm();
            let name = core::str::from_utf8(&comm[..])
                .unwrap_or("")
                .trim_end_matches('\0');
            let _ = writeln!(
                w2,
                "  MHUNT-ROW tid={} comm='{}' sys={} a0={:#x} on_word={} wrap={} [+{:#x},{:#x},{:#x}] ng={} [+{:#x},{:#x}] hook={} [+{:#x},{:#x}]",
                tk.pid(),
                name,
                sys,
                a0,
                on_word,
                nw,
                wrap_hits[0],
                wrap_hits[1],
                wrap_hits[2],
                nn,
                ng_hits[0],
                ng_hits[1],
                nh,
                hook_hits[0],
                hook_hits[1],
            );
            if on_word && nw >= 2 && v_rec.is_none() {
                v_rec = Some(tk.pid() as u64);
            }
            if !on_word && v_stuck.is_none() {
                v_stuck = Some((tk.pid() as u64, sys, a0));
            }
        });
        match (v_rec, v_stuck) {
            (Some(tid), _) => {
                let _ = writeln!(w2, "MHUNT-VERDICT recursive-holder tid={} (>=2 WRAP frames while parked on __malloc_lock)", tid);
            }
            (None, Some((tid, sys, a0))) => {
                let _ = writeln!(w2, "MHUNT-VERDICT stuck-holder tid={} sys={} a0={:#x} (allocator frames while parked OFF the lock)", tid, sys, a0);
            }
            (None, None) => {
                let _ = writeln!(w2, "MHUNT-VERDICT none: thread-side holders acquitted (holder exited holding)");
            }
        }
        let _ = writeln!(w2, "MHUNT end rows={} waiters_on_word={}", rows, waiters);
    }
}

/// 118th (mouse-endgame): malloc-lock holder hunt. The standing word
/// 0x3fbf13e6a8 is ld-musl's `__malloc_lock[0]` (bss static at vaddr
/// 0x13e6a8 of the oh image's ld-musl-x86_64.so.1 — verified by
/// disassembly: mallocng's LOCK(__malloc_lock) per musl glue.h, where
/// rdlock==wrlock==exclusive; bit31 = held, low count = 1+waiters).
/// ls1 froze foundation's copy at <=12.43 s (val=0x8000001d = held +
/// 28 waiters, low count tracking arrivals 1:1) with NO thread parked
/// inside malloc on any syscall and NO running thread — so the holder
/// either exited while holding, or is a recursive-malloc self-deadlock
/// (the holder parks on its own lock as the first waiter). This scan
/// walks every same-tgid thread's user stack and counts frames inside
/// the allocator code ranges of that image (base 0x3fbf000000, fixed
/// by the loader):
///   WRAP [0x3fbf12adb0, 0x3fbf12b4b8) — malloc/calloc/free/realloc/
///        aligned_alloc/malloc_usable_size wrappers
///   NG   [0x3fbf0ba000, 0x3fbf0bd000) — mallocng internals (incl.
///        the unlock helper with the cmpxchg ladder)
///   HOOK [0x3fbf12bb00, 0x3fbf12c500) — OH malloc-hook cluster
/// Verdict rules:
///   - a WAITER on this word with >= 2 WRAP frames = recursive
///     self-deadlock holder (named);
///   - a thread parked OFF the word (any other syscall/futex) with a
///     WRAP or NG frame = holder stuck inside malloc (named);
///   - all waiters carry <= 1 WRAP frame and no off-word thread hits
///     = thread-side holders acquitted (holder exited holding).
/// Same for_each_task_try + page-walk cost class as the reader scan.
///
/// 120th ROOT-CAUSE CORRECTION #2 (cotask-decode): the classifier
/// constants since the 118th compared runtime addresses against
/// `MUSL_BASE = 0x3fbf_0000_0000` — 256x ABOVE the real mapping (the
/// loader maps ld-musl at 0x3fbf000000-class; the extra two hex digits
/// were a unit error, same family as the 32-bit page-mask bug). The
/// WRAP/NG/HOOK/LCKHLP windows sat in an unmapped gap below every real
/// code address and NEVER matched — the thread-side holder hunt has
/// been inert since birth (its "acquitted" verdicts were vacuous).
/// Robust replacement: classify by FILE OFFSET against the tgid's LIVE
/// ld-musl exec VMA (file_off = addr - vma.start + vma.offset); the
/// windows below are the BuildID 99627cfd ELF-vaddr windows shifted by
/// the exec LOAD's vaddr-file delta (0x1000):
///   WRAP_F  [0x129db0, 0x12a4b8) malloc/calloc/realloc/free wrappers
///   NG_F    [0xb9000,  0xbc000)  mallocng internals (incl. unlock)
///   HOOK_F  [0x12ab00, 0x12b500) OH malloc-hook cluster
///   LCKHLP_F [0x115b40, 0x115e00) LOCK() futex helper (parked
///                                  waiters' saved rip lands here)
pub fn musl_anchor(tgid: u32) -> Option<(u64, u64)> {
    let mut anchor: Option<(u64, u64)> = None;
    crate::sched::hotpath::for_each_task_try(|task| {
        if anchor.is_some() {
            return;
        }
        // SAFETY: diagnostic read under the for_each_task contract.
        let tk = crate::sched::r#unsafe::task_shared(task);
        if tk.tgid() as u32 != tgid {
            return;
        }
        if let Some(aspace) = tk.address_space() {
            if let Some(mgr) = aspace.vma_try_read() {
                for vma in mgr.iter() {
                    if !vma.flags().is_executable() {
                        continue;
                    }
                    let name = aspace
                        .try_get_vma_file(vma.start().as_usize())
                        .map(|f| f.path())
                        .unwrap_or_default();
                    let base: alloc::string::String = name
                        .rsplit('/')
                        .next()
                        .unwrap_or("-")
                        .chars()
                        .take(28)
                        .collect();
                    if base.starts_with("ld-musl") {
                        anchor = Some((
                            vma.start().as_usize() as u64,
                            vma.offset() as u64,
                        ));
                        break;
                    }
                }
            }
        }
    });
    anchor
}

/// File-offset classifier for the windows documented on musl_anchor.
/// Returns (wrap, ng, hook, lckhlp) membership as a packed u8.
#[inline]
pub fn musl_off_class(anchor: (u64, u64), x: u64) -> u8 {
    let Some(off) = x.checked_sub(anchor.0).map(|d| d + anchor.1) else {
        return 0;
    };
    if (0x129db0..0x12a4b8).contains(&off) {
        1
    } else if (0xb9000..0xbc000).contains(&off) {
        2
    } else if (0x12ab00..0x12b500).contains(&off) {
        3
    } else if (0x115b40..0x115e00).contains(&off) {
        4
    } else {
        0
    }
}

pub fn malloc_holder_scan(tgid: u32, word: u64) {
    use core::fmt::Write as _;
    #[cfg(feature = "x86_64")]
    const FUTEX_SYSNO: u64 = 202;
    #[cfg(not(feature = "x86_64"))]
    const FUTEX_SYSNO: u64 = 0;
    let mut w = crate::debug::types::ConsoleWriter::new();
    let _ = writeln!(w, "MHUNT tgid={} word={:#x}:", tgid, word);
    let mut rows = 0u32;
    let mut waiters = 0u32;
    let mut verdict_recursive: Option<u64> = None;
    let mut verdict_stuck: Option<(u64, u64, u64)> = None;
    let musl = musl_anchor(tgid);
    crate::sched::hotpath::for_each_task_try(|t| {
        if rows >= 24 {
            return;
        }
        // SAFETY: pid-hash walk contract; reads confined to pt-regs and
        // the task's own user pages.
        let tk = crate::sched::r#unsafe::task_shared(t);
        if tk.tgid() as u32 != tgid {
            return;
        }
        let Some(top) = tk.kernel_stack_top_checked() else {
            return;
        };
        #[cfg(feature = "x86_64")]
        let (sys, a0, rsp) = {
            let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
            { (regs.orig_rax, regs.rdi, regs.rsp) }
        };
        #[cfg(not(feature = "x86_64"))]
        let (sys, a0, rsp) = (0u64, 0u64, 0u64);
        if rsp == 0 || rsp > 0x7fff_ffff_ffff {
            return;
        }
        let on_word = sys == FUTEX_SYSNO && a0 == word;
        if on_word {
            waiters += 1;
        }
        let mut wrap_hits: [u64; 3] = [0; 3];
        let mut ng_hits: [u64; 2] = [0; 2];
        let mut hook_hits: [u64; 2] = [0; 2];
        let mut nw = 0usize;
        let mut nn = 0usize;
        let mut nh = 0usize;
        // 4096 words (32 KB) — the proven reader-scan window depth.
        for i in 0..4096u64 {
            let va = rsp + 8 * i;
            if va & 0xfff > 0xff8 {
                continue; // page-cross split — skip
            }
            let lo = rux::sync::r#unsafe::read_u32_via_task(t, va as usize).unwrap_or(0) as u64
                | ((rux::sync::r#unsafe::read_u32_via_task(t, va as usize + 4).unwrap_or(0) as u64) << 32);
            let cls = musl.map(|a| musl_off_class(a, lo)).unwrap_or(0);
            if cls == 1 {
                if nw < 3 {
                    wrap_hits[nw] = lo;
                }
                nw += 1;
            } else if cls == 2 {
                if nn < 2 {
                    ng_hits[nn] = lo;
                }
                nn += 1;
            } else if cls == 3 {
                if nh < 2 {
                    hook_hits[nh] = lo;
                }
                nh += 1;
            }
        }
        if nw == 0 && nn == 0 {
            return;
        }
        rows += 1;
        let comm = tk.comm();
        let name = core::str::from_utf8(&comm[..])
            .unwrap_or("")
            .trim_end_matches('\0');
        let _ = writeln!(
            w,
            "  MHUNT-ROW tid={} comm='{}' sys={} a0={:#x} on_word={} wrap={} [+{:#x},{:#x},{:#x}] ng={} [+{:#x},{:#x}] hook={} [+{:#x},{:#x}]",
            tk.pid(),
            name,
            sys,
            a0,
            on_word,
            nw,
            wrap_hits[0],
            wrap_hits[1],
            wrap_hits[2],
            nn,
            ng_hits[0],
            ng_hits[1],
            nh,
            hook_hits[0],
            hook_hits[1],
        );
        if on_word && nw >= 2 && verdict_recursive.is_none() {
            verdict_recursive = Some(tk.pid() as u64);
        }
        if !on_word && verdict_stuck.is_none() {
            verdict_stuck = Some((tk.pid() as u64, sys, a0));
        }
    });
    match (verdict_recursive, verdict_stuck) {
        (Some(tid), _) => {
            let _ = writeln!(w, "MHUNT-VERDICT recursive-holder tid={} (>=2 WRAP frames while parked on __malloc_lock)", tid);
        }
        (None, Some((tid, sys, a0))) => {
            let _ = writeln!(w, "MHUNT-VERDICT stuck-holder tid={} sys={} a0={:#x} (allocator frames while parked OFF the lock)", tid, sys, a0);
        }
        (None, None) => {
            let _ = writeln!(w, "MHUNT-VERDICT none: all allocator-frame threads are single-frame waiters; thread-side holders acquitted (holder exited holding)");
        }
    }
    let _ = writeln!(w, "MHUNT end rows={} waiters_on_word={}", rows, waiters);
    // 120th (cotask-decode): the standing-word adoption is the LATE arm
    // of the same hunt the early trigger (do_futex CTD-EARLY) starts at
    // first-waiter +2 s; ctd_decode self-latches once per tgid so the
    // late call is a no-op when the early one already ran, and the
    // backstop when the early window missed (boot without fuxstuck, or
    // a wedge whose waiters never cross the >=2-waiter early gate).
    ctd_decode(tgid, word, "L");
}

/// 120th (cotask-decode): FFRT CoTask saved-stack decoder for the
/// malloc-lock wedge. The 118th proved foundation's __malloc_lock
/// (0x3fbf13e6a8, ld-musl fixed base) latches held-forever with the
/// low count climbing 1:1 with parked waiters, no unlocking write, no
/// wake attempt, and NO thread carrying allocator frames — the open
/// hypotheses are (a) a suspended FFRT coroutine holding through a
/// yield, (b) a holder that exited holding, (c) a holder descheduled
/// mid-critical-section in a state no prior instrument names (page
/// fault inside mallocng, signal-handler recursion). This decoder runs
/// the full structural pass ONCE per tgid at the earliest wedge signal:
///
///  Phase T — every same-tgid thread: syscall, park uaddr, USER RIP
///    (pt_regs) classified by module. A thread faulting INSIDE the
///    allocator (RIP in mallocng/wrapper code, no syscall — the IO-stuck
///    or demand-paging-stuck holder shape) is named here; prior taskdump
///    rows only printed the syscall.
///  Phase W — every worker's ExecuteCtx (musl TSD scan, the oh68
///    contract): ctx, ctx->task, task label, ThreadWait word ctx+224.
///    The ctx pointers are also the heap anchors for Phase C.
///  Phase C — CoTask heap sweep over anchor-derived windows (each ctx
///    +-1.5 MB merged; the 118th slab sweep anchored on the ld-musl
///    mapping itself and never reached foundation's 0x2aab... arena).
///    Structural shape (BuildID-verified layouts, libffrt bb6920ed):
///      CoTask: type@0x18 in 0..=4 (ffrt task types), gid@0x20 small,
///        preStatus@0x30/curStatus@0x34 (uint8 enums, cur<=11),
///        createTime@0x60, executeTime@0x68, fromTid@0x70, label@0xd8
///        (libc++ SSO or long), we@0x100 (WaitEntry: ring links +0x8,
///        task@0x20), wue@0x128 (WaitUntilEntry*: status@+0x30,
///        deadline@+0x38), coRoutine@0x130, runningTid@0x140,
///        threadMode_@0x1a9; QueueTask adds handler_@0x1b8,
///        nextTask_@0x1d8.
///      CoRoutine: status@0 (0=uninit 1=not_finish 2=running), task@0x10
///        backref, fiber RSP@+0x48 RIP@+0x50, stkMem.size@0x68,
///        stkMem.magic@0x70 == 0x7bcdabcdabcdabcd (the shape test),
///        stack [co+0x78, co+0x78+size).
///    Every suspended CoTask (saved RIP not at libffrt's context_entry
///    trampoline, file offset 0x86a24 = queued-fresh) prints label +
///    saved RIP/RSP as module file offsets plus a walked stack; any
///    stack word EQUAL to the malloc word (W!) or inside the ld-musl
///    allocator/lock-helper VA ranges (A!) flags a holder candidate,
///    which the verdict line names. TaskStatus strings from
///    ffrt types.h: 0 PND 1 ENQ 2 DEQ 3 SUB 4 RDY 5 POP 6 EXE 7 THB
///    (THREAD_BLOCK) 8 CRB (COROUTINE_BLOCK) 9 FIN 10 WRS 11 CAN.
///
/// All prints are SHORT self-contained rows (the 118th's single-line
/// OH68 dump was garbled beyond recovery by concurrent census printers
/// on the same wedge). Cost: ~30 tasks x (2 walks + 64-word TSD scan)
/// + sweep budget 500k single-word reads — the proven reader-scan class
/// (for_each_task_try, no pid-bucket locks held). x86_64-only content.
pub fn ctd_decode(tgid: u32, word: u64, tag: &str) {
    use core::fmt::Write as _;
    use core::sync::atomic::Ordering::Relaxed;
    if tgid == 0 {
        return;
    }
    // once per tgid (the early arm carries the freshest holder state;
    // re-runs would only re-print the same frozen graph). ctf1 live
    // fire: the OLD single global slot was eaten by hilogd's benign
    // 2 s boot-contention at 21.9 s, so foundation's trigger 0.3 s
    // later decoded nothing — per-tgid slots now (16; overflow falls
    // back to skip, the census late arm still prints for latecomers).
    const DONE_SLOTS: usize = 16;
    static DONE_TGIDS: [core::sync::atomic::AtomicU32; DONE_SLOTS] =
        [const { core::sync::atomic::AtomicU32::new(0) }; DONE_SLOTS];
    let mut claimed = false;
    for i in 0..DONE_SLOTS {
        let s = DONE_TGIDS[i].load(Relaxed);
        if s == tgid {
            return;
        }
        if s == 0 && !claimed {
            claimed = DONE_TGIDS[i]
                .compare_exchange(0, tgid, Relaxed, Relaxed)
                .is_ok();
            if !claimed {
                // lost the race and it wasn't us — recheck this slot
                if DONE_TGIDS[i].load(Relaxed) == tgid {
                    return;
                }
            }
        }
    }
    if !claimed {
        return;
    }
    let mut w = crate::debug::types::ConsoleWriter::new();
    let _ = writeln!(
        w,
        "CTD-HEAD {} tgid={} word={:#x} jif={}",
        tag,
        tgid,
        word,
        crate::timer::get_jiffies()
    );
    if cfg!(not(target_arch = "x86_64")) {
        let _ = writeln!(w, "CTD-END (x86_64-only)");
        return;
    }
    #[cfg(target_arch = "x86_64")]
    {
        // Phase 0: any same-tgid root (address spaces are shared).
        let mut root: u64 = 0;
        crate::sched::hotpath::for_each_task_try(|task| {
            if root != 0 {
                return;
            }
            // SAFETY: diagnostic read under the for_each_task contract.
            let tk = crate::sched::r#unsafe::task_shared(task);
            if tk.tgid() as u32 != tgid {
                return;
            }
            if let Some(aspace) = tk.address_space() {
                root = aspace.root_ppn();
            }
        });
        if root == 0 {
            let _ = writeln!(w, "CTD-END no-root");
            return;
        }
        let rd = |va: u64| -> Option<u64> { rd_word(root, va) };
        let rd32at = |va: u64| -> Option<u32> {
            rd(va & !7).map(|w| (w >> ((va & 4) * 8)) as u32)
        };
        // Phase 1: exec-VMA table (module classifier). 96 entries (was
        // 48 — ctf7: foundation maps more than 48 exec VMAs and the
        // cap lost libffrt, so CoYield rips printed as `?`), one
        // task's map (all threads share it).
        let mut exec_vmas: alloc::vec::Vec<(u64, u64, u64, alloc::string::String)> =
            alloc::vec::Vec::new();
        crate::sched::hotpath::for_each_task_try(|task| {
            if !exec_vmas.is_empty() {
                return;
            }
            // SAFETY: diagnostic read under the for_each_task contract.
            let tk = crate::sched::r#unsafe::task_shared(task);
            if tk.tgid() as u32 != tgid {
                return;
            }
            if let Some(aspace) = tk.address_space() {
                if let Some(mgr) = aspace.vma_try_read() {
                    for vma in mgr.iter() {
                        if vma.flags().is_executable()
                            && exec_vmas.len() < 96
                        {
                            let name = aspace
                                .try_get_vma_file(vma.start().as_usize())
                                .map(|f| f.path())
                                .unwrap_or_default();
                            let base: alloc::string::String = name
                                .rsplit('/')
                                .next()
                                .unwrap_or("-")
                                .chars()
                                .take(28)
                                .collect();
                            exec_vmas.push((
                                vma.start().as_usize() as u64,
                                vma.end().as_usize() as u64,
                                vma.offset() as u64,
                                base,
                            ));
                        }
                    }
                }
            }
        });
        let word_off = |x: u64| -> Option<(usize, u64)> {
            for (i, &(s, e, f, _)) in exec_vmas.iter().enumerate() {
                if x >= s && x < e {
                    return Some((i, x - s + f));
                }
            }
            None
        };
        // libffrt's vma index + the ld-musl vma index (name match; the
        // OH image maps each exactly once per process).
        let mut ffrt_ix = usize::MAX;
        let mut musl_ix = usize::MAX;
        for (i, &(_, _, _, ref n)) in exec_vmas.iter().enumerate() {
            if n.starts_with("libffrt") && ffrt_ix == usize::MAX {
                ffrt_ix = i;
            }
            if n.starts_with("ld-musl") && musl_ix == usize::MAX {
                musl_ix = i;
            }
        }
        for (i, &(s, e, f, ref n)) in exec_vmas.iter().enumerate() {
            let _ = writeln!(w, "CTD-VMA v{}={:#x}-{:#x}/{:#x}/{}", i, s, e, f, n);
        }
        // ld-musl allocator frame classifier — 120th CORRECTION #2: the
        // old MUSL_BASE(0x3fbf_0000_0000)+window form was 256x above
        // the real mapping and never matched (see musl_anchor doc).
        // Classify by FILE OFFSET through this decode's own VMA table
        // (musl_ix); windows are BuildID-pinned ELF vaddrs - 0x1000.
        let musl_off = |x: u64| -> Option<u64> {
            match word_off(x) {
                Some((m, o)) if m == musl_ix => Some(o),
                _ => None,
            }
        };
        let in_alloc = |x: u64| -> bool {
            match musl_off(x) {
                Some(o) => {
                    (0x129db0..0x12a4b8).contains(&o)  // WRAP wrappers
                        || (0xb9000..0xbc000).contains(&o)  // NG internals
                        || (0x12ab00..0x12b500).contains(&o) // HOOK cluster
                }
                None => false,
            }
        };
        let in_lckhlp = |x: u64| -> bool {
            matches!(musl_off(x), Some(o) if (0x115b40..0x115e00).contains(&o))
        };
        // Phase T + W: thread table and worker ctx sweep in ONE pass
        // (both walk the same task set; ctx discovery also collects the
        // Phase C anchors). Thread rows print FIRST (holder-nameable
        // even if the sweep later trips its budget).
        let mut anchors: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
        let mut nthreads = 0u32;
        let mut alloc_rip_tids: alloc::vec::Vec<(u32, u64)> = alloc::vec::Vec::new();
        crate::sched::hotpath::for_each_task_try(|task| {
            if nthreads >= 40 {
                return;
            }
            // SAFETY: diagnostic read under the for_each_task contract;
            // reads confined to pt-regs, TLS and the task's own pages.
            let tk = crate::sched::r#unsafe::task_shared(task);
            if tk.tgid() as u32 != tgid {
                return;
            }
            nthreads += 1;
            let tid = tk.pid() as u32;
            let comm = tk.comm();
            let name = core::str::from_utf8(&comm[..])
                .unwrap_or("")
                .trim_end_matches('\0');
            let Some(top) = tk.kernel_stack_top_checked() else {
                return;
            };
            let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
            let (sys, a0, rip) =
                { (regs.orig_rax, regs.rdi, regs.rip) };
            let ripoff = match word_off(rip) {
                Some((m, o)) => alloc::format!("v{}+{:#x}", m, o),
                None => alloc::string::String::from("?"),
            };
            let mut flags = alloc::string::String::new();
            if in_alloc(rip) || in_lckhlp(rip) {
                let _ = write!(flags, " ALLOC-RIP");
                alloc_rip_tids.push((tid, rip));
            }
            if sys == 202 && a0 != 0 && (a0 as u64 >> 47) == 0 && a0 as u64 > 0x1000 {
                // heap-class park words anchor Phase C into the C++
                // object arena (the ld-musl word itself is not heap).
                if (a0 as u64) >= 0x1000_0000_0000 && (a0 as u64) < 0x7f00_0000_0000 {
                    anchors.push(a0 as u64);
                }
            }
            let _ = writeln!(
                w,
                "CTD-THR tid={} '{}' sys={} a0={:#x} rip={:#x}@{}{}",
                tid,
                name,
                sys,
                a0,
                rip,
                ripoff,
                flags
            );
            // Phase W: this thread's ExecuteCtx (TSD scan, oh68 key
            // cache). Thread-mode tasks (ctx->task set while parked
            // inside Execute()) carry the task identity here.
            let fs = tk.thread().fs_base;
            let ctx = oh68_ctx_of(root, fs, tid);
            if ctx != 0 {
                let taskp = rd(ctx + 16).unwrap_or(0);
                let (gid, ty, cur, lbl) = if taskp > 0x1000 && (taskp >> 47) == 0 {
                    (
                        rd(taskp + 0x20).unwrap_or(0),
                        rd32at(taskp + 0x18).unwrap_or(0xffff),
                        (rd(taskp + 0x30).unwrap_or(0) >> 32) & 0xff,
                        ctd_label(&rd, taskp + 0xd8),
                    )
                } else {
                    (0, 0xffff, 0xff, alloc::string::String::new())
                };
                let _ = writeln!(
                    w,
                    "CTD-WRK tid={} ctx={:#x} task={:#x} gid={:#x} ty={} cur={} lbl=\"{}\" parkw={:#x}",
                    tid,
                    ctx,
                    taskp,
                    gid,
                    ty,
                    cur,
                    lbl,
                    rd(ctx + 224).unwrap_or(0)
                );
                anchors.push(ctx);
                if taskp > 0x1000 && (taskp >> 47) == 0 {
                    anchors.push(taskp);
                }
            }
        });
        let _ = writeln!(w, "CTD-THREADS n={}", nthreads);
        let otask = FUXWATCH_OWNER_TASK.load(Relaxed);
        if otask > 0x1000 && (otask >> 47) == 0 {
            // the acquire-time latch (process-blind — validate it belongs
            // to THIS tgid by page-table reachability before anchoring)
            if rd(otask + 0x20).is_some() {
                let _ = writeln!(
                    w,
                    "CTD-OTASK latch={:#x} (cross-process latch, reachable)", otask
                );
                anchors.push(otask);
            }
        }
        // Phase C: CoTask sweep over merged anchor windows.
        anchors.sort_unstable();
        anchors.dedup();
        const WIN: u64 = 0x180000; // +-1.5 MB
        let mut wins: alloc::vec::Vec<(u64, u64)> = alloc::vec::Vec::new();
        for &a in anchors.iter() {
            if a < 0x1000 || (a >> 47) != 0 {
                continue;
            }
            // skip the fixed loader mapping (not a CoTask arena)
            if a >= 0x3fbf_0000_0000 && a < 0x3fbf_4000_0000 {
                continue;
            }
            let (lo, hi) = (a.saturating_sub(WIN), a + WIN);
            match wins.last_mut() {
                Some(last) if lo <= last.1 => {
                    if hi > last.1 {
                        last.1 = hi;
                    }
                }
                _ => wins.push((lo, hi)),
            }
            if wins.len() >= 4 {
                break;
            }
        }
        // ctf1 live fire: 500k words exhausted inside the second 3 MB
        // anchor window (cts=0, budget_left=0) — hilogd's arena walk
        // never finished. 2M words covers all four merged windows.
        let mut budget: u64 = 2_000_000;
        let mut found = 0u32;
        let mut suspended_n = 0u32;
        let mut holder_n = 0u32;
        for &(lo, hi) in wins.iter() {
            if budget == 0 || found >= 40 {
                break;
            }
            let _ = writeln!(w, "CTD-SWEEP win={:#x}-{:#x}", lo, hi);
            let mut base = lo;
            while base < hi && budget > 0 && found < 40 {
                budget -= 1;
                // type@gid window: one read at +0x18 covers type(4)+pad;
                // most words fail here.
                let Some(w18) = rd(base + 0x18) else {
                    base += 8;
                    continue;
                };
                let ty = w18 as u32;
                if ty > 4 {
                    base += 8;
                    continue;
                }
                let gid = match rd(base + 0x20) {
                    Some(g) if (1..=0x400000).contains(&g) => g,
                    _ => {
                        base += 8;
                        continue;
                    }
                };
                let co = match rd(base + 0x130) {
                    Some(c) if c > 0x1000 && (c >> 47) == 0 => c,
                    _ => {
                        base += 8;
                        continue;
                    }
                };
                // CoRoutine stkMem magic — the strong shape test.
                if rd(co + 0x70) != Some(0x7bcd_abcd_abcd_abcd) {
                    base += 8;
                    continue;
                }
                let w30 = rd(base + 0x30).unwrap_or(0);
                let cur = (w30 >> 32) & 0xff;
                let pre = w30 & 0xff;
                if cur > 11 || pre > 11 {
                    base += 8;
                    continue;
                }
                found += 1;
                let label = ctd_label(&rd, base + 0xd8);
                let rsp = rd(co + 0x48).unwrap_or(0);
                let rip = rd(co + 0x50).unwrap_or(0);
                let stk_sz = rd(co + 0x68).unwrap_or(0).min(0x1800);
                let ripoff = match word_off(rip) {
                    Some((m, o)) => alloc::format!("v{}+{:#x}", m, o),
                    None => alloc::string::String::from("?"),
                };
                // queued-fresh: saved RIP at libffrt context_entry.
                let fresh = match word_off(rip) {
                    Some((m, o)) if m == ffrt_ix && o == 0x86a24 => true,
                    _ => false,
                };
                let wue = rd(base + 0x128).unwrap_or(0);
                let rtid = rd(base + 0x140).unwrap_or(0);
                let thr_m = rd(base + 0x1a9).map(|v| v & 0xff).unwrap_or(0xff);
                let _ = writeln!(
                    w,
                    "CTD-CT tp={:#x} g{:#x} ty{} cur{}(pre{}) ct={} ex={} ft={} rtid={} thrM={} wue={:#x} rip={}{} lbl=\"{}\"",
                    base,
                    gid,
                    ty,
                    cur,
                    pre,
                    rd(base + 0x60).unwrap_or(0),
                    rd(base + 0x68).unwrap_or(0),
                    rd32at(base + 0x70).unwrap_or(0xffff),
                    rtid,
                    thr_m,
                    wue,
                    ripoff,
                    if fresh { " FRESH" } else { "" },
                    label
                );
                if fresh || rsp == 0 || (rsp >> 47) != 0 || rsp & 7 != 0 {
                    base += 8;
                    continue;
                }
                suspended_n += 1;
                // saved-stack walk: every exec-VMA word is a candidate
                // return address; W! = malloc-word data ref, A! =
                // allocator code frame. Also flag the saved RIP itself.
                let mut frames: alloc::vec::Vec<(usize, u64)> = alloc::vec::Vec::new();
                let mut wrefs = 0u32;
                let mut aframes: alloc::vec::Vec<u64> = alloc::vec::Vec::new();
                let lo_stk = co + 0x78;
                let hi_stk = co + 0x78 + stk_sz;
                let top = (rsp + 0x1800).min(hi_stk);
                if rsp >= lo_stk {
                    let mut va = rsp;
                    while va < top {
                        if let Some(x) = rd(va) {
                            if x == word {
                                wrefs += 1;
                            } else if in_alloc(x) || in_lckhlp(x) {
                                if aframes.len() < 6 {
                                    aframes.push(
                                        musl_off(x).unwrap_or(0),
                                    );
                                }
                            } else if let Some((m, o)) = word_off(x) {
                                if frames.len() < 20 {
                                    frames.push((m, o));
                                }
                            }
                        }
                        va += 8;
                    }
                }
                let rip_alloc = in_alloc(rip) || in_lckhlp(rip);
                let mut fs = alloc::string::String::new();
                for (m, o) in frames.iter() {
                    let _ = write!(fs, " v{}:{:#x}", m, o);
                }
                if !aframes.is_empty() || rip_alloc || wrefs > 0 {
                    let _ = write!(fs, " W!={} A!=+{:x?}", wrefs, aframes);
                    if rip_alloc {
                        let _ = write!(fs, " RIPALLOC");
                    }
                }
                let _ = writeln!(
                    w,
                    "CTD-ST tp={:#x} rsp={:#x} rip={:#x} st={}{}",
                    base,
                    rsp,
                    rip,
                    rd32at(co).unwrap_or(0xffff),
                    fs
                );
                // resumer structures: the we ring (which sync object's
                // waiter list this task is on) and the wue timeout state
                // (status INIT + long-past deadline = the delayed-worker
                // never fired; the offline pass compares against boot
                // steady-clock ns).
                let wen = rd(base + 0x108).unwrap_or(0);
                if wen != 0 && (wen >> 47) == 0 && wen != base + 0x100 {
                    let mut ring = alloc::string::String::new();
                    let mut cur = wen;
                    let mut hops = 0u32;
                    while hops < 8 {
                        let _ = write!(ring, " {:#x}", cur);
                        match rd(cur + 0x8) {
                            Some(nx) if nx != 0 && (nx >> 47) == 0 && nx != base + 0x100 => {
                                cur = nx;
                                hops += 1;
                            }
                            _ => break,
                        }
                    }
                    let _ = writeln!(w, "CTD-WE tp={:#x} ring:{}", base, ring);
                }
                if wue > 0x1000 && (wue >> 47) == 0 {
                    let _ = writeln!(
                        w,
                        "CTD-WUE tp={:#x} wue={:#x} status={} dl={:#x}",
                        base,
                        wue,
                        rd32at(wue + 0x30).unwrap_or(0xffff),
                        rd(wue + 0x38).unwrap_or(0)
                    );
                }
                // QueueTask: the SQ that owns this task is its resumer.
                if ty == 3 {
                    let handler = rd(base + 0x1b8).unwrap_or(0);
                    if handler > 0x1000 && (handler >> 47) == 0 {
                        let queue = rd(handler + 8).unwrap_or(0);
                        if queue > 0x1000 && (queue >> 47) == 0 {
                            let _ = writeln!(
                                w,
                                "CTD-QT tp={:#x} h={:#x} q={:#x} active={} mapsz={} nxt={:#x} qname=\"{}\"",
                                base,
                                handler,
                                queue,
                                rd(queue + 78).unwrap_or(0) & 0xff,
                                rd(queue + 96).unwrap_or(0),
                                rd(base + 0x1d8).unwrap_or(0),
                                ctd_label(&rd, queue + 0xc8)
                            );
                        }
                    }
                }
                if rip_alloc || (!aframes.is_empty() && wrefs > 0) {
                    holder_n += 1;
                    let _ = writeln!(
                        w,
                        "CTD-HOLDER tp={:#x} gid={:#x} lbl=\"{}\" rip={} wrefs={} aframes={:?}",
                        base,
                        gid,
                        label,
                        ripoff,
                        wrefs,
                        aframes
                    );
                }
                base += 8;
            }
        }
        let _ = writeln!(
            w,
            "CTD-END {} tgid={} threads={} cts={} susp={} holders={} wins={} budget_left={}",
            tag,
            tgid,
            nthreads,
            found,
            suspended_n,
            holder_n,
            wins.len(),
            budget
        );
        for &(tid, rip) in alloc_rip_tids.iter() {
            let _ = writeln!(
                w,
                "CTD-ALLOCRIP tid={} rip={:#x} (thread executing allocator code, no syscall)",
                tid,
                rip
            );
        }
    }
}

/// 120th (cotask-decode): libc++ std::string decode at `va` — SSO
/// (size in byte0>>1, chars at +1) or long form (size@+8, ptr@+16).
/// Bounded 64 bytes; the label identifies the FFRT task (queue names,
/// sq_collect lambdas, service entry labels).
fn ctd_label(
    rd: &dyn Fn(u64) -> Option<u64>,
    va: u64,
) -> alloc::string::String {
    let w0 = match rd(va) {
        Some(v) => v,
        None => return alloc::string::String::new(),
    };
    let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    if w0 & 1 == 0 {
        let n = ((w0 & 0xff) >> 1) as usize;
        if n > 0 && n <= 22 {
            let w1 = rd(va + 8).unwrap_or(0);
            let w2 = rd(va + 16).unwrap_or(0);
            let buf = [w0, w1, w2];
            for i in 1..=n {
                let b = ((buf[i / 8] >> (8 * (i % 8))) & 0xff) as u8;
                if !(0x20..0x7f).contains(&b) {
                    return alloc::string::String::new();
                }
                out.push(b);
            }
        }
    } else {
        let size = rd(va + 8).unwrap_or(0);
        let ptr = rd(va + 16).unwrap_or(0);
        if size <= 64 && (ptr >> 47) == 0 && ptr > 0x1000 {
            for i in 0..size as u64 {
                match rd(ptr + i).map(|x| (x & 0xff) as u8) {
                    Some(b) if (0x20..0x7f).contains(&b) => out.push(b),
                    _ => break,
                }
            }
        }
    }
    alloc::string::String::from(core::str::from_utf8(&out).unwrap_or(""))
}

/// Cluster of futex waiters on one (uaddr, mm): how many, which tgid and
/// one sample task (for the page-table read).
struct WordCluster {
    uaddr: usize,
    mm: usize,
    tgid: u32,
    n: usize,
    sample: *mut Task,
}

fn waiter_clusters(min: usize) -> alloc::vec::Vec<WordCluster> {
    let mut out: alloc::vec::Vec<WordCluster> = alloc::vec::Vec::new();
    for b in HASH_HEADS.iter() {
        let h = b.lock_irqsave();
        let mut idx = h.head;
        'chain: while let Some(i) = idx {
            let (entry, next) = {
                let slot = WAITER_POOL[i].lock_irqsave();
                match slot.as_ref() {
                    Some(w) if !w.task.is_null() => {
                        let t = w.task;
                        let pid = crate::sched::r#unsafe::task_shared(t).pid();
                        let live = core::ptr::eq(
                            crate::sched::hotpath::find_task_by_pid(pid),
                            t,
                        );
                        if live {
                            (
                                Some((w.key.uaddr, w.key.mm, crate::sched::r#unsafe::task_shared(t).tgid() as u32, t)),
                                w.next,
                            )
                        } else {
                            (None, w.next)
                        }
                    }
                    Some(w2) => (None, w2.next),
                    None => (None, None),
                }
            };
            if let Some((uaddr, mm, tgid, t)) = entry {
                if let Some(c) = out.iter_mut().find(|c| c.uaddr == uaddr && c.mm == mm) {
                    c.n += 1;
                } else {
                    out.push(WordCluster { uaddr, mm, tgid, n: 1, sample: t });
                }
            }
            match next {
                Some(nx) => idx = Some(nx),
                None => break 'chain,
            }
        }
        drop(h);
    }
    out.retain(|c| c.n >= min);
    out
}

/// musl-rwlock-shaped word: waiters bit set and a sane holder count
/// (writer-held 0x7fffffff excluded — that shape drains normally).
fn rwlock_frozen_shape(v: u32) -> bool {
    let readers = v & 0x7fffffff;
    v & 0x80000000 != 0 && readers >= 1 && readers < 0x40000000
}

/// Called from the binder stall scan at khungtaskd cadence (process
/// context, no locks held). Tracks reader-count evolution on contended
/// rwlock words, watches the frozen one's futex traffic, diffs the tgid's
/// live tids, and fires a one-shot full-tgid park-stack dump.
pub fn freeze_scan(now: u64, hz: u64) {
    if hz == 0 {
        return;
    }
    // RETIRED (probe-retire): the whole scan is rwlock-freeze hunt
    // instrumentation (word adoption, inflation curves, waiter decodes,
    // park-stack dumps) and it SELF-ARMS on ordinary glib/udisksd mutex
    // contention — every ub2404 boot printed ~150 RWFRZ lines in the
    // settled phase. The entire body (adoption included — adopting a
    // word also fills a RWFRZ_WATCH slot, arming the per-op tracer)
    // now sits behind the existing futex-hunt switch (rux:futextrace (retired debug=futextrace)).
    // The only functional action inside is watch-slot bookkeeping; the
    // explicit rwfrzwatch= cmdline pre-arm above stays available.
    if !crate::trace::EVENT_FUTEX_MISS.armed() {
        return;
    }
    // sc83: scanner liveness heartbeat — one line per minute. sc82 lost
    // khungtaskd silently ~2 min after adoption (stall_report's 500 ms
    // irq-off WORLD window); without this line that death is invisible.
    // Serial-cleanliness: the heartbeat joins the wwcensus family so
    // ordinary boots print nothing periodic.
    if crate::trace::EVENT_WWCENSUS.armed() {
        static BEAT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
        let last = BEAT.load(core::sync::atomic::Ordering::Relaxed);
        if now.saturating_sub(last) >= 60 * hz {
            BEAT.store(now, core::sync::atomic::Ordering::Relaxed);
            // HOTPATH-PRINT: switch/cmdline-gated (off by default).
            crate::pr_warn!("RWFRZ: scan alive at {}s", now / hz);
        }
    }
    // ---- adopt / refresh the watched word ----
    let mut adopted = false;
    for c in waiter_clusters(1) {
        let v = match rux::sync::r#unsafe::read_u32_via_task(c.sample, c.uaddr) {
            Some(v) => v,
            None => continue,
        };
        let readers = v & 0x7fffffff;
        // Formation watch (sc82: threshold lowered 6 -> 2, per-word table
        // with delta): the inflation curve decides leak vs corruption.
        if readers >= 2
            && readers < 0x40000000
            && c.uaddr != RWFRZ_UADDR.load(core::sync::atomic::Ordering::Relaxed)
        {
            let mut hl = RWFRZ_HIGHTAB.lock_irqsave();
            let (ref mut hn, ref mut htab) = *hl;
            let slot = (0..*hn).find(|&i| htab[i].uaddr == c.uaddr);
            match slot {
                Some(i) => {
                    if now.saturating_sub(htab[i].at) >= 60 * hz {
                        let prev_r = htab[i].lastv & 0x7fffffff;
                        let cur_r = v & 0x7fffffff;
                        let unchanged = cur_r == prev_r;
                        htab[i].lastv = v;
                        htab[i].at = now;
                        // rwlock-livelock lane: name the sample waiter (the
                        // parked task the cluster was keyed on) so the dump
                        // answers "who waits" without a second probe.
                        let (sample_tid, sample_state) = if c.sample.is_null() {
                            (0u32, 0u32)
                        } else {
                            // SAFETY: read-only pid/state of a task still
                            // linked in a futex bucket (waiter_clusters ran
                            // under the bucket lock when it sampled).
                            {
                    let cs = crate::sched::r#unsafe::task_shared(c.sample);
                    (cs.pid(), cs.state().bits())
                }
                        };
                        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                        crate::pr_warn!(
                            "RWFRZ-HIGH uaddr={:#x} tgid {} waiters={} word={:#x} readers={} (was {}, d{:+}) wtid={} wstate={:#x}",
                            c.uaddr,
                            c.tgid,
                            c.n,
                            v,
                            cur_r,
                            prev_r,
                            cur_r as i64 - prev_r as i64,
                            sample_tid,
                            sample_state
                        );
                        // rwlock-livelock: the parked sample waiter's user
                        // frame — comm + the SYSCALL-ENTRY registers still
                        // live in pt_regs while the task sleeps in
                        // futex_wait. On x86_64: rip = return address
                        // after the syscall instruction (names the libc
                        // wait site), rsi = op, rdx = val (the EXPECTED
                        // word value the waiter parked on — decides which
                        // glibc rwlock wait site this is: __readers vs
                        // __wrphase_futex vs __writers_futex), r8 = uaddr2.
                        // This mirrors HUNG-CTX's frame read; printed as a
                        // separate line so the RWFRZ-HIGH shape stays
                        // diffable against older serials.
                        if !c.sample.is_null() {
                            // SAFETY: read-only diagnostics of a task still
                            // linked in a futex bucket (sampled under the
                            // bucket lock by waiter_clusters).
                            let tk = crate::sched::r#unsafe::task_shared(c.sample);
                            let cb = tk.comm();
                            let cl = cb.iter().position(|&b| b == 0).unwrap_or(16);
                            let comm = core::str::from_utf8(&cb[..cl]).unwrap_or("?");
                            // The process name: the tgid leader's comm
                            // (thread comms like 'pool-spawner' name the
                            // GLib role, the leader names the daemon).
                            let lead = crate::sched::hotpath::find_task_by_pid(c.tgid);
                            let leader_comm = if lead.is_null() {
                                "?"
                            } else {
                                // SAFETY: read-only comm of the tgid leader
                                // found by pid-hash lookup.
                                let lb = crate::sched::r#unsafe::task_shared(lead).comm();
                                let ll = lb.iter().position(|&b| b == 0).unwrap_or(16);
                                core::str::from_utf8(&lb[..ll]).unwrap_or("?")
                            };
                            let pr = tk.pt_regs();
                            if !pr.is_null() {
                                #[cfg(feature = "x86_64")]
                                let (urip, uop, uval) = match tk.pt_regs_ref() {
                                    Some(r) => (r.rip, r.rsi as i64, r.rdx),
                                    None => (0, 0, 0),
                                };
                                #[cfg(feature = "riscv64")]
                                let (urip, uop, uval) = match tk.pt_regs_ref() {
                                    Some(r) => (r.epc, r.a1 as i64, r.a2),
                                    None => (0, 0, 0),
                                };
                                #[cfg(not(any(feature = "riscv64", feature = "x86_64")))]
                                let (urip, uop, uval) = (0u64, 0i64, 0u32);
                                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                                crate::pr_warn!(
                                    "RWFRZ-WAITER tid={} comm='{}' tgid={} leader='{}' unrip={:#x} unrop={} unval={:#x}",
                                    sample_tid,
                                    comm,
                                    c.tgid,
                                    leader_comm,
                                    urip,
                                    uop,
                                    uval
                                );
                            }
                        }
                        // rwlock-livelock: dump the LOCK STRUCT once per
                        // stable window — 33 u32 words from uaddr-64 to
                        // uaddr+64 through the sample task's page table.
                        // The word pattern identifies the primitive where
                        // guesswork cannot: pthread_mutex (__lock 0/1/2 at
                        // +0, owner tid at +8), pthread_rwlock (__readers
                        // flags 1/2/4 at +0 with count<<3, __wrphase_futex
                        // at +8, __writers_futex at +12), pthread_cond
                        // (__wseq u64 at +0, __g1_orig_size at +8, __wrefs
                        // at +12, __g_signals[2] at +16/+20). The observed
                        // park vals 2->3 fit glibc rwlock __wrphase_futex
                        // but the observed WAKE nr=1 fits cond/mutex —
                        // only the raw bytes settle it.
                        if unchanged && !c.sample.is_null() {
                            let base = c.uaddr.saturating_sub(64) & !3;
                            // One pr_warn PER ROW: a single multi-line
                            // string loses its tail to console interleaving
                            // (observed in boot F: the dump died after 7 of
                            // 33 words every window).
                            for row in 0..5usize {
                                let mut words = alloc::string::String::new();
                                use core::fmt::Write as _;
                                for k in 0..8usize {
                                    let idx = row * 8 + k;
                                    if idx >= 33 {
                                        break;
                                    }
                                    let va = base + idx * 4;
                                    match rux::sync::r#unsafe::read_u32_via_task(c.sample, va) {
                                        Some(wv) => {
                                            let _ = write!(
                                                words,
                                                " {:+#x}={:#x}",
                                                va as i64 - c.uaddr as i64,
                                                wv
                                            );
                                        }
                                        None => {
                                            let _ = write!(
                                                words,
                                                " {:+#x}=X",
                                                va as i64 - c.uaddr as i64
                                            );
                                        }
                                    }
                                }
                                if !words.is_empty() {
                                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                                    crate::pr_warn!(
                                        "RWFRZ-MEM{} uaddr={:#x} tgid={}:{}",
                                        row,
                                        c.uaddr,
                                        c.tgid,
                                        words
                                    );
                                }
                            }
                        }
                        // rwlock-livelock lane (glibc shape): the ub2404
                        // freeze is word=0x2 — NO waiters bit, so the musl
                        // RWFRZ-PARK hook (which requires val&0x80000000)
                        // never latches it and the sc185 oplog stays dark.
                        // A cluster word that kept >=2 readers for a full
                        // 60 s census window with a live waiter is frozen
                        // by any protocol's lights: latch it into the
                        // existing RWFRZ-WATCH table so every subsequent
                        // futex op on the word prints (WAIT vs WAKE vs
                        // silence — the (a)/(b)/(c) verdict data). Same
                        // first-free-slot pattern as the RWFRZ-PARK latch.
                        if unchanged {
                            let watched = RWFRZ_WATCH.iter().any(|w| {
                                w.load(core::sync::atomic::Ordering::Relaxed)
                                    == c.uaddr
                            });
                            if !watched {
                                for w in RWFRZ_WATCH.iter() {
                                    if w.load(core::sync::atomic::Ordering::Relaxed) == 0 {
                                        w.store(c.uaddr, core::sync::atomic::Ordering::Relaxed);
                                        // HOTPATH-PRINT: census/forensic emitter (1 Hz heartbeat- or wedge-gated caller; cold path).
                                        crate::pr_warn!(
                                            "RWFRZ-WATCH-LATCH uaddr={:#x} tgid {} (stable 60s, readers={})",
                                            c.uaddr,
                                            c.tgid,
                                            cur_r
                                        );
                                        break;
                                    }
                                }
                                // hw1 (huks-wedge): run the reader-identity
                                // scan for EVERY standing word whose live
                                // waiter never owes a binder txn — the TXN
                                // census park-row path never fires for those
                                // (hw2w1: zero FUTEX park rows, yet 12+
                                // standing 0x2/0x80000010 words). The scan
                                // names the tgid's threads that carry the
                                // word (or its owner object) on their stacks
                                // — the holder candidates. Once per word
                                // (16-slot table), ConsoleWriter inside.
                                {
                                    const HW_SCAN_SZ: usize = 16;
                                    static HW_SCANNED: [core::sync::atomic::AtomicU64;
                                        HW_SCAN_SZ] =
                                        [const {
                                            core::sync::atomic::AtomicU64::new(0)
                                        }; HW_SCAN_SZ];
                                    let key = ((c.tgid as u64) << 48)
                                        | ((c.uaddr as u64) & 0xffff_ffff_ffff);
                                    let mut hit = false;
                                    for s in HW_SCANNED.iter() {
                                        if s.load(core::sync::atomic::Ordering::Relaxed)
                                            == key
                                        {
                                            hit = true;
                                            break;
                                        }
                                    }
                                    if !hit {
                                        for s in HW_SCANNED.iter() {
                                            if s
                                                .compare_exchange(
                                                    0,
                                                    key,
                                                    core::sync::atomic::Ordering::Relaxed,
                                                    core::sync::atomic::Ordering::Relaxed,
                                                )
                                                .is_ok()
                                            {
                                                rwfrz_reader_scan(c.tgid, c.uaddr as u64);
                                                break;
                                            }
                                        }
                                    }
                                }
                                // rwlock-livelock: one-shot full-tgid task
                                // dump AT THE LATCH (the musl-shape FROZEN
                                // dump never fires for the glibc word=0x2
                                // shape — no waiters bit). Every task's
                                // last syscall + rip + arg0 + kernel frames:
                                // the frozen word's HOLDER (or the thread
                                // blocked mid-critical-section in the
                                // kernel) shows up as sys=clone/exec/read
                                // with frames naming the stuck path.
                                static TGID_DUMPED: core::sync::atomic::AtomicU32 =
                                    core::sync::atomic::AtomicU32::new(0);
                                if TGID_DUMPED
                                    .compare_exchange(
                                        0,
                                        c.tgid,
                                        core::sync::atomic::Ordering::Relaxed,
                                        core::sync::atomic::Ordering::Relaxed,
                                    )
                                    .is_ok()
                                {
                                    exit_ring_dump_tgid(c.tgid);
                                    let n = rwfz_dump_tgid(c.tgid, 32, true);
                                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                                    crate::pr_warn!(
                                        "RWFRZ: GBC-FROZEN tgid {} uaddr={:#x} word={:#x} — dumped {} tasks",
                                        c.tgid,
                                        c.uaddr,
                                        v,
                                        n
                                    );
                                    // rwlock-livelock: the sample waiter's
                                    // USER return-address chain (call-site-
                                    // validated, vma+file-offset annotated)
                                    // — names the exact libc/GLib wait site
                                    // (mutex vs cond vs rwlock path) when
                                    // symbolized against the image's libs.
                                    if !c.sample.is_null() {
                                        apppark_rets(c.sample);
                                    }
                                }
                            }
                        }
                    }
                }
                None => {
                    // sc83: evict-by-readers when full — sc82 filled all 8
                    // slots with transient readers=2 words at boot and the
                    // real frozen word could never enter the table.
                    let mut slot = if *hn < 8 { *hn } else { 0 };
                    if *hn >= 8 {
                        let mut min_r = u32::MAX;
                        for i in 0..8 {
                            let r = htab[i].lastv & 0x7fffffff;
                            if r < min_r {
                                min_r = r;
                                slot = i;
                            }
                        }
                        if readers <= min_r {
                            slot = usize::MAX; // nothing worth evicting
                        }
                    }
                    if slot != usize::MAX {
                        htab[slot] = HighWord {
                            uaddr: c.uaddr,
                            tgid: c.tgid,
                            lastv: v,
                            at: now,
                        };
                        if *hn < 8 {
                            *hn += 1;
                        }
                        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                        crate::pr_warn!(
                            "RWFRZ-HIGH uaddr={:#x} tgid {} waiters={} word={:#x} readers={} (first seen)",
                            c.uaddr,
                            c.tgid,
                            c.n,
                            v,
                            readers
                        );
                        // rwlock-livelock: latch the futex-op watch AT
                        // FIRST SIGHT, not after a stable 60s window —
                        // the holder's final unlock (the (a)-verdict
                        // datum) can land inside that first minute and
                        // was invisible in boots E/F/G.
                        {
                            let watched = RWFRZ_WATCH.iter().any(|w| {
                                w.load(core::sync::atomic::Ordering::Relaxed) == c.uaddr
                            });
                            if !watched {
                                for w in RWFRZ_WATCH.iter() {
                                    if w.load(core::sync::atomic::Ordering::Relaxed) == 0 {
                                        w.store(c.uaddr, core::sync::atomic::Ordering::Relaxed);
                                        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                                        crate::pr_warn!(
                                            "RWFRZ-WATCH-EARLY uaddr={:#x} tgid {} word={:#x}",
                                            c.uaddr,
                                            c.tgid,
                                            v
                                        );
                                        break;
                                    }
                                }
                                // ...and run the one-shot full-tgid task
                                // dump HERE too (boot J: the early latch
                                // made the 60s-stable LATCH's !watched
                                // guard false and the dump never fired).
                                static TGID_DUMPED_E: core::sync::atomic::AtomicU32 =
                                    core::sync::atomic::AtomicU32::new(0);
                                if TGID_DUMPED_E
                                    .compare_exchange(
                                        0,
                                        c.tgid,
                                        core::sync::atomic::Ordering::Relaxed,
                                        core::sync::atomic::Ordering::Relaxed,
                                    )
                                    .is_ok()
                                {
                                    exit_ring_dump_tgid(c.tgid);
                                    let n = rwfz_dump_tgid(c.tgid, 32, true);
                                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                                    crate::pr_warn!(
                                        "RWFRZ: GBC-EARLY tgid {} uaddr={:#x} word={:#x} — dumped {} tasks",
                                        c.tgid,
                                        c.uaddr,
                                        v,
                                        n
                                    );
                                    if !c.sample.is_null() {
                                        apppark_rets(c.sample);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            drop(hl);
        }
        if adopted || c.n < 3 || !rwlock_frozen_shape(v) {
            // 82nd: even before adoption, a DEEP freeze (8+ readers) on
            // any clustered word runs the sqwalk slice — ohw139's wedge
            // face (BWR-parked looper) never takes the census park-row
            // path, so the audit-site trigger alone would miss it. The
            // word-keyed statics make this idempotent across triggers.
            // 83rd: `!adopted` DROPPED — same-pass re-adoption of an
            // earlier word starved later frozen words forever (ohw150:
            // foundation's 0x7ff000cb1444 froze at 188 s and NEVER got
            // a pass because the 853 word was re-adopted first in every
            // freeze_scan pass). sqwalk's own sticky/drain latch
            // arbitrates; duplicate triggers are harmless.
            if readers >= 8 {
                sqwalk_step(c.tgid, c.uaddr as u64);
            } else if c.n >= 12 {
                // 83rd: a DEEP cluster on a non-rwlock word — ohw152's
                // face (16 samgr threads never-woken on a cond-class
                // ...194 word, wake[ever=0] for 415 s, no 0x80000010
                // rwlock ever forming). The raw phase maps that word's
                // holder graph in the host tgid.
                sqwalk_step_deep(c.tgid, c.uaddr as u64, true);
            }
            continue;
        }
        let cur_u = RWFRZ_UADDR.load(core::sync::atomic::Ordering::Relaxed);
        if cur_u == c.uaddr {
            // 82nd: the ADOPTED word is the wedge's own — drive its
            // sqwalk slice here too (ohw146: once adopted, only the
            // 60 s census path called sqwalk and the 40-slice pass
            // could not finish before the boot died).
            if readers >= 8 {
                sqwalk_step(c.tgid, c.uaddr as u64);
            }
            continue; // already watched (delta path below)
        }
        RWFRZ_UADDR.store(c.uaddr, core::sync::atomic::Ordering::Relaxed);
        RWFRZ_MM.store(c.mm, core::sync::atomic::Ordering::Relaxed);
        RWFRZ_TGID.store(c.tgid, core::sync::atomic::Ordering::Relaxed);
        RWFRZ_FIRST.store(now, core::sync::atomic::Ordering::Relaxed);
        RWFRZ_LASTVAL.store(v, core::sync::atomic::Ordering::Relaxed);
        RWFRZ_LASTVAL_AT.store(now, core::sync::atomic::Ordering::Relaxed);
        RWFRZ_DUMPED.store(false, core::sync::atomic::Ordering::Relaxed);
        RWFRZ_OPLOG.store(0, core::sync::atomic::Ordering::Relaxed);
        adopted = true;
        // HOTPATH-PRINT: census/forensic emitter (1 Hz heartbeat- or wedge-gated caller; cold path).
        crate::pr_warn!(
            "RWFRZ: watching uaddr={:#x} mm={:#x} tgid {} waiters={} word={:#x} readers={}",
            c.uaddr,
            c.mm,
            c.tgid,
            c.n,
            v,
            readers
        );
    }
    // ---- 83rd: same-offset park family (the ohw154 samgr face) ----
    // samgr's loopers each park on a DIFFERENT word that shares one page
    // offset (...194-class: one waiter per word, so no cluster reaches
    // the n>=12 deep trigger). A tgid with >=4 distinct parked words at
    // the same offset = one object CLASS frozen across its instances —
    // arm the raw scan for its oldest member; SQW_DEEP keeps the pass
    // sticky (the word's value says nothing about liveness).
    {
        // (offset, tgid) -> (count, first_uaddr)
        let mut fam: [(u32, u32, u32, usize); 8] = [(0, 0, 0, 0); 8];
        let mut armed = false;
        for c in waiter_clusters(1) {
            let off = (c.uaddr & 0xfff) as u32;
            let mut slot = None;
            for i in 0..8 {
                if fam[i].0 == off && fam[i].1 == c.tgid {
                    slot = Some(i);
                    break;
                }
                if fam[i].0 == 0 && slot.is_none() {
                    slot = Some(i);
                }
            }
            if let Some(i) = slot {
                if fam[i].0 == off && fam[i].1 == c.tgid {
                    fam[i].2 += 1;
                } else {
                    fam[i] = (off, c.tgid, 1, c.uaddr);
                }
            }
        }
        for &(off, t, n, u) in fam.iter() {
            if off != 0 && n >= 4 && !armed {
                armed = true;
                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                crate::pr_warn!(
                    "RWFRZ-OFFFAM offset={:#x} tgid={} words={} sample={:#x} (same-offset park family)",
                    off,
                    t,
                    n,
                    u
                );
                sqwalk_step_deep(t, u as u64, true);
            }
        }
    }
    let uaddr = RWFRZ_UADDR.load(core::sync::atomic::Ordering::Relaxed);
    let tgid = RWFRZ_TGID.load(core::sync::atomic::Ordering::Relaxed);
    // ---- tid census diff (leak forensics) ----
    // sc82: run from boot start on the samgr tgid (334 on the scgold
    // image, stable across sc74..sc81) so pre-adoption reader exits are
    // timestamped too; switches to the adopted tgid once a word is taken.
    const EARLY_SAMGR_TGID: u32 = 334;
    let census_tgid = if uaddr != 0 { tgid } else { EARLY_SAMGR_TGID };
    {
        let seen_cell: core::cell::RefCell<(usize, [u32; 64])> =
            core::cell::RefCell::new((0, [0; 64]));
        crate::sched::hotpath::for_each_task(|t| {
            // SAFETY: callback runs under the pid-hash bucket lock (same
            // contract as sleep_snapshot); reads confined to pid fields.
            let tk = crate::sched::r#unsafe::task_shared(t);
            let idx = {
                let mut b = seen_cell.borrow_mut();
                if tk.tgid() as u32 == census_tgid && b.0 < 64 {
                    let i = b.0;
                    b.0 += 1;
                    Some(i)
                } else {
                    None
                }
            };
            if let Some(i) = idx {
                seen_cell.borrow_mut().1[i] = tk.pid();
            }
        });
        let (k, seen) = seen_cell.into_inner();
        let mut g = RWFRZ_TIDS.lock_irqsave();
        let (ref mut prev_n, ref mut prev) = *g;
        let prev_tgid = RWFRZ_TIDS_CENSUS_TGID.swap(census_tgid, core::sync::atomic::Ordering::Relaxed);
        if prev_tgid != census_tgid {
            // Baseline switch (early-watch -> adopted word): reset silently.
            *prev_n = 0;
        }
        for i in 0..*prev_n {
            let tid = prev[i];
            if tid != 0 && !seen[..k].contains(&tid) {
                // HOTPATH-PRINT: census/forensic emitter (1 Hz heartbeat- or wedge-gated caller; cold path).
                crate::pr_warn!(
                    "RWFRZ: TID-VANISHED tgid {} tid {} (reader-exit window; word={:#x})",
                    census_tgid,
                    tid,
                    RWFRZ_LASTVAL.load(core::sync::atomic::Ordering::Relaxed)
                );
            }
        }
        for s in seen[..k].iter() {
            if !prev[..*prev_n].contains(s) {
                // HOTPATH-PRINT: census/forensic emitter (1 Hz heartbeat- or wedge-gated caller; cold path).
                crate::pr_warn!("RWFRZ: TID-NEW tgid {} tid {}", census_tgid, s);
            }
        }
        prev[..k.min(64)].copy_from_slice(&seen[..k.min(64)]);
        *prev_n = k;
        drop(g);
    }
    if uaddr == 0 {
        return;
    }
    // Re-read through any live waiter of the tgid (sample may have exited).
    let mut word_now: Option<u32> = None;
    for c in waiter_clusters(1) {
        if c.uaddr == uaddr && c.mm == RWFRZ_MM.load(core::sync::atomic::Ordering::Relaxed) {
            word_now = rux::sync::r#unsafe::read_u32_via_task(c.sample, c.uaddr);
            break;
        }
    }
    // ---- word delta ----
    if let Some(v) = word_now {
        let prev = RWFRZ_LASTVAL.load(core::sync::atomic::Ordering::Relaxed);
        if v != prev {
            RWFRZ_LASTVAL.store(v, core::sync::atomic::Ordering::Relaxed);
            RWFRZ_LASTVAL_AT.store(now, core::sync::atomic::Ordering::Relaxed);
            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
            crate::pr_warn!(
                "RWFRZ: word delta {:#x} -> {:#x} (readers {} -> {}) at {}s",
                prev,
                v,
                prev & 0x7fffffff,
                v & 0x7fffffff,
                now / hz
            );
        } else {
            // Heartbeat once a minute on the frozen word: proves the count
            // is stuck, not merely unobserved. Does NOT touch LASTVAL_AT —
            // that stamp is the freeze-stability clock for the dump gate.
            let at = RWFRZ_BEAT.load(core::sync::atomic::Ordering::Relaxed);
            if now.saturating_sub(at) >= 60 * hz {
                RWFRZ_BEAT.store(now, core::sync::atomic::Ordering::Relaxed);
                let mm = RWFRZ_MM.load(core::sync::atomic::Ordering::Relaxed);
                let wcount = waiter_clusters(1)
                    .iter()
                    .find(|c| c.uaddr == uaddr && c.mm == mm)
                    .map(|c| c.n)
                    .unwrap_or(0);
                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                crate::pr_warn!(
                    "RWFRZ: word stuck {:#x} readers={} kernel_waiters={} tgid {} age {}s",
                    v,
                    v & 0x7fffffff,
                    wcount,
                    tgid,
                    now.saturating_sub(RWFRZ_FIRST.load(core::sync::atomic::Ordering::Relaxed)) / hz
                );
            }
        }
    }
    // ---- one-shot full-tgid dump ----
    // sc83: fire AT ADOPTION (gate removed). sc82 showed the scanner can
    // die on the NEXT scan (stall_report's own WORLD-guarded dump runs
    // ~500 ms with irqs off under load; khungtaskd never scanned again)
    // — waiting 120 s for stability loses the dump entirely. The frozen
    // shape (waiters>=3 + WAITERS|N-readers word) is already
    // cross-boot-stable; stability-waiting buys nothing.
    if !RWFRZ_DUMPED.load(core::sync::atomic::Ordering::Relaxed) {
        RWFRZ_DUMPED.store(true, core::sync::atomic::Ordering::Relaxed);
        let v = RWFRZ_LASTVAL.load(core::sync::atomic::Ordering::Relaxed);
        // neighbors through the first waiter we can find: +4/+8 disambiguate
        // the primitive (musl rwlock: waiter-count/shared-flag; musl mutex:
        // __m_type/__m_count — a type constant at +4 means this is NOT an
        // rwlock and the "readers" decode changes meaning entirely).
        let mm = RWFRZ_MM.load(core::sync::atomic::Ordering::Relaxed);
        let mut nb = (0u32, 0u32, 0u32, 0u32);
        for c in waiter_clusters(1) {
            if c.uaddr == uaddr && c.mm == mm {
                nb = (
                    rux::sync::r#unsafe::read_u32_via_task(c.sample, uaddr + 4).unwrap_or(0),
                    rux::sync::r#unsafe::read_u32_via_task(c.sample, uaddr + 8).unwrap_or(0),
                    rux::sync::r#unsafe::read_u32_via_task(c.sample, uaddr + 12).unwrap_or(0),
                    rux::sync::r#unsafe::read_u32_via_task(c.sample, uaddr + 16).unwrap_or(0),
                );
                break;
            }
        }
        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
        crate::pr_warn!(
            "RWFRZ: FROZEN tgid {} uaddr={:#x} word={:#x} readers={} waiters_bit={} nb+4={:#x} nb+8={:#x} nb+12={:#x} nb+16={:#x} age {}s — full tgid dump",
            tgid,
            uaddr,
            v,
            v & 0x7fffffff,
            v >> 31,
            nb.0,
            nb.1,
            nb.2,
            nb.3,
            now.saturating_sub(RWFRZ_FIRST.load(core::sync::atomic::Ordering::Relaxed)) / hz
        );
        exit_ring_dump_tgid(tgid);
        // 71st: priority ring — this is THE founder-evidence dump.
        let n = rwfz_dump_tgid(tgid, 32, true);
        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
        crate::pr_warn!("RWFRZ: tgid {} enumerated {} tasks", tgid, n);
        rwfz_neighbor_dump(tgid, uaddr);
        // sc82: the freeze correlates with OTHER processes showing the
        // same 16-reader shape (sc81: tgid 716 & 791). Dump up to 2 more
        // of those tgid's in the same pass — one common holder pattern
        // would explain all of them; distinct stacks point at a shared
        // library-level bug instead.
        let mut extra = 0;
        for c in waiter_clusters(1) {
            if extra >= 2 {
                break;
            }
            if c.tgid == tgid || c.n < 1 {
                continue;
            }
            let v = match rux::sync::r#unsafe::read_u32_via_task(c.sample, c.uaddr) {
                Some(v) => v,
                None => continue,
            };
            let r = v & 0x7fffffff;
            if v & 0x80000000 != 0 && r >= 8 && r < 0x40000000 {
                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                crate::pr_warn!(
                    "RWFRZ: EXTRA-FROZEN tgid {} uaddr={:#x} word={:#x} readers={} waiters={} — same-shape dump",
                    c.tgid,
                    c.uaddr,
                    v,
                    r,
                    c.n
                );
                exit_ring_dump_tgid(c.tgid);
                let en = rwfz_dump_tgid(c.tgid, 16, false);
                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                crate::pr_warn!("RWFRZ: EXTRA tgid {} enumerated {} tasks", c.tgid, en);
                rwfz_neighbor_dump(c.tgid, c.uaddr);
                extra += 1;
            }
        }
    }
}

/// sc89: dump the recursion-holder candidate. Session K (sc86): every
/// FROZEN dump's main thread parks on the SIBLING lock 0x94 below the
/// frozen word (same static object) while the frozen word claims 16
/// readers that exceed the process's live-thread count — a single
/// thread holding 16 recursive lock_shared scopes and then parking on
/// the second lock. This walks `tgid`'s tasks for a blocked futex whose
/// arg0 sits in [frozen-0x98, frozen-0x90] and dumps it with the
/// 64-slot exec-filtered window at a 24-frame budget — enough to carry
/// a 16-deep recursive chain plus its entry frames.
fn rwfz_neighbor_dump(tgid: u32, frozen_uaddr: usize) {
    #[cfg(feature = "x86_64")]
    const FUTEX_SYSNO: u64 = 202;
    #[cfg(feature = "riscv64")]
    const FUTEX_SYSNO: u64 = 98;
    #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
    const FUTEX_SYSNO: u64 = 0;
    crate::sched::hotpath::for_each_task(|t| {
        // SAFETY: same contract as rwfz_dump_tgid (pid-hash walk, reads
        // confined to the pt-regs entry frame).
        let tk = crate::sched::r#unsafe::task_shared(t);
        if tk.tgid() as u32 != tgid {
            return;
        }
        // Stack-freed guard (KILLDUMP census race class): the pid-hash
        // walk can visit a task whose exit already freed its stack.
        let Some(top) = tk.kernel_stack_top_checked() else {
            return;
        };
        #[cfg(feature = "x86_64")]
        let (sys, a0) = {
            let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
            // SAFETY: diagnostic read of this task's entry frame.
            { (regs.orig_rax, regs.rdi) }
        };
        #[cfg(feature = "riscv64")]
        let (sys, a0) = {
            let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
            // SAFETY: same contract, riscv64.
            { (regs.a7, regs.a0) }
        };
        #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
        let (sys, a0) = (0u64, 0u64);
        if sys != FUTEX_SYSNO || a0 == 0 || a0 > frozen_uaddr as u64 {
            return;
        }
        let delta = frozen_uaddr - a0 as usize;
        if (0x90..=0x98).contains(&delta) {
            let comm = tk.comm();
            let name = core::str::from_utf8(&comm[..])
                .unwrap_or("")
                .trim_end_matches('\0');
            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
            crate::pr_warn!(
                "RWFRZ: NBR-PARK tgid {} tid {} comm='{}' parked on frozen-{:#x} (a0={:#x}) — recursion-holder candidate, deep stack",
                tgid,
                tk.pid(),
                name,
                delta,
                a0
            );
            // sc91: read the WORD the holder parks on (plus neighbors).
            // If it is a held pthread mutex the word IS the owner tid —
            // naming the true terminal blocker. If it is a shared_mutex
            // __state_ we see the writer bit. The frozen word itself is
            // 0x80000010-shaped, but this sibling lock's shape was never
            // captured (sc89/sc90 dumps only read the frozen word).
            let w0 = rux::sync::r#unsafe::read_u32_via_task(t, a0 as usize).unwrap_or(0);
            let wp4 = rux::sync::r#unsafe::read_u32_via_task(t, a0 as usize + 4).unwrap_or(0);
            let wm4 = rux::sync::r#unsafe::read_u32_via_task(t, a0 as usize - 4).unwrap_or(0);
            let wp8 = rux::sync::r#unsafe::read_u32_via_task(t, a0 as usize + 8).unwrap_or(0);
            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
            crate::pr_warn!(
                "RWFRZ: NBR-WORD a0={:#x} word={:#x} +4={:#x} +8={:#x} -4={:#x}",
                a0,
                w0,
                wp4,
                wp8,
                wm4
            );
            // sc91: RAW stack-slot window — VMA metadata cannot identify
            // loader-mapped code frames (musl closes the fd AND the
            // offset bookkeeping is zeroed for merged LOAD mappings —
            // sc90 nbr dumps recovered only data-segment pointers).
            // Print the slot VALUES themselves, compactly; offline
            // analysis matches them against lib files (return addresses
            // sit right after `call` opcodes in exec LOADs).
            use core::fmt::Write;
            let sp = {
                // `top` is the guarded value from the closure head
                // (kernel_stack_top_checked) — never raw here.
                #[cfg(feature = "x86_64")]
                let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
                #[cfg(feature = "riscv64")]
                let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
                // SAFETY: diagnostic read of this task's entry frame.
                #[cfg(feature = "x86_64")]
                {
                    regs.rsp
                }
                // SAFETY: same contract, riscv64.
                #[cfg(feature = "riscv64")]
                {
                    regs.sp
                }
                #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
                {
                    0u64
                }
            };
            let mut line = alloc::string::String::new();
            let mut n = 0u32;
            for i in 0..256u64 {
                let Some(v) = (rux::sync::r#unsafe::read_u32_via_task(t, (sp + 8 * i) as usize))
                    .map(|lo| lo as u64)
                    .and_then(|lo| {
                        (rux::sync::r#unsafe::read_u32_via_task(t, (sp + 8 * i + 4) as usize))
                            .map(|hi| lo | ((hi as u64) << 32))
                    })
                else {
                    continue;
                };
                if v >= 0x1000 && v <= 0x7fff_ffff_ffff {
                    let _ = write!(line, " {}:{}", i, v);
                    n += 1;
                    if n % 8 == 0 {
                        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                        crate::pr_warn!("RWFRZ: NBR-RAW{}", line);
                        line.clear();
                    }
                }
            }
            if !line.is_empty() {
                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                crate::pr_warn!("RWFRZ: NBR-RAW{}", line);
            }
            // SAFETY: t is a live Task from the pid-hash walk.
            // sc90: 256-slot window (2 KiB) — the 16-deep recursive
            // chain sits ~1-2 KiB above rsp; 24 exec-file frames catch
            // repeated fileoffs at fixed deltas (the recursion cycle).
            // Emitted twice (nbr/nbr2): concurrent console traffic
            // splices long multi-line dumps mid-line (sc89 lost the
            // holder's chain between frame[2] and frame[17]); two
            // passes rarely lose the same line twice.
            rwfz_stack_frames(t, "nbr", 24, 256, false);
            rwfz_stack_frames(t, "nbr2", 24, 256, false);
        }
    });
}

/// sc82: one tgid's full park-stack dump (comm/state/syscall/rip + 8
/// byte-verified user-stack frames per thread). Pure diagnostics.
pub fn rwfz_dump_tgid(tgid: u32, max_tasks: usize, prio: bool) -> usize {
    let count_cell: core::cell::Cell<usize> = core::cell::Cell::new(0);
    crate::sched::hotpath::for_each_task(|t| {
        // SAFETY: same contract as sleep_snapshot.
        let tk = crate::sched::r#unsafe::task_shared(t);
        if tk.tgid() as u32 != tgid || count_cell.get() >= max_tasks {
            return;
        }
        count_cell.set(count_cell.get() + 1);
        // Stack-freed guard (KILLDUMP census race class): the pid-hash
        // walk can visit a task whose exit already freed its stack.
        let Some(top) = tk.kernel_stack_top_checked() else {
            return;
        };
        #[cfg(feature = "x86_64")]
        let (rip, sys, a0) = {
            let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
            // SAFETY: diagnostic read of this task's entry frame.
            { (regs.rip, regs.orig_rax, regs.rdi) }
        };
        #[cfg(feature = "riscv64")]
        let (rip, sys, a0) = {
            let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
            // SAFETY: same contract, riscv64.
            { (regs.epc, regs.a7, regs.a0) }
        };
        #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
        let (rip, sys, a0) = (0u64, 0u64, 0u64);
        let comm = tk.comm();
        let name = core::str::from_utf8(&comm[..])
            .unwrap_or("")
            .trim_end_matches('\0');
        rwfz_emit_prio(
            &alloc::format!(
                "RWFRZ: tid {} comm='{}' state={:#x} sys={:#x} rip={:#x} a0={:#x}",
                tk.pid(),
                name,
                tk.state().bits(),
                sys,
                rip,
                a0
            ),
            prio,
        );
        // SAFETY: t is a live Task from the pid-hash walk.
        rwfz_stack_frames(t, "task", 8, 64, prio);
    });
    count_cell.get()
}

/// Census-side latch+arm for a sync-holder futex park word (see the
/// FUXPARK statics). Called every audit burst; stores unconditionally so
/// a mid-park word SHIFT is tracked (the immediate wake verdict follows
/// the live word). Watch arming: slot 1 when free; a DIFFERENT page than
/// slot 1 takes slot 2 (the PTE watch is per page — a same-page shift is
/// already covered by slot 1's page plus the FW-PG page-mate log).
/// rux:fuxstuck (retired debug=fuxstuck)-gated by the caller.
///
/// huks-wedge lane (hw1): STICKY watch slots. The 81st re-targeted slot
/// 1 on every latch, so with several stalled tgids the write watch kept
/// re-arming the winner of the moment (hw1w1: huks 0x..64224 -> huks
/// 0x..19224 -> foundation 0x..4e24 within 70 ms; huks's word lost its
/// slot before any write could be logged). The verdict fields
/// (FUXPARK_WORD/TID/TGID) still follow the newest row exactly as the
/// 81st had them; only the SLOTS are now assigned to the first two
/// DISTINCT stalled tgids and re-targeted only on a same-tgid page move
/// (with the done-set re-opened so the moved page actually arms — the
/// old code never reset it on the slot-1 move path).
pub fn fuxpark_latch(word: u64, tid: u32, tgid: u32) {
    use core::sync::atomic::Ordering::Relaxed;
    if word == 0 || word < 0x1000 {
        return;
    }
    let prev = FUXPARK_WORD.swap(word, Relaxed);
    FUXPARK_TID.store(tid, Relaxed);
    FUXPARK_TGID.store(tgid, Relaxed);
    if prev == 0 {
        FUXPARK_J.store(crate::timer::get_jiffies() as u32, Relaxed);
    }
    // ---- watch-slot assignment (sticky, per tgid) ----
    let arm_print = |slot: u8, w: u64| {
        let mut cw = crate::debug::types::ConsoleWriter::new();
        use core::fmt::Write as _;
        let _ = writeln!(
            cw,
            "FUXPARK-ARM s{} word={:#x} tid={} tgid={} (sync-holder park{})",
            slot,
            w,
            tid,
            tgid,
            if slot == 1 { "" } else { ", sticky" }
        );
        crate::debug::hotpath::ensure_defer_thread();
    };
    let reopen_done_set = || {
        for d in FUXWATCH_MM_DONE.iter() {
            d.store(0, Relaxed);
        }
    };
    let s1 = FUXWATCH_ADDR.load(Relaxed);
    let s1_t = FUXWATCH1_TGID.load(Relaxed);
    if s1 == 0 {
        if FUXWATCH_ADDR
            .compare_exchange(0, word, Relaxed, Relaxed)
            .is_ok()
        {
            FUXWATCH1_TGID.store(tgid, Relaxed);
            // ConsoleWriter, not pr_warn — the censusd audit context
            // drops printk (ohw128-134: this print never appeared
            // while the arm provably happened).
            arm_print(1, word);
        }
        return;
    }
    if s1_t == tgid {
        // Same-tgid word (mid-park shift or a sibling word): follow only
        // a PAGE move (a same-page shift is already covered by the RO
        // page plus the FW-PG page-mate log).
        if (s1 & !0xfff) != (word & !0xfff) {
            // huks-wedge lane (hw2): retire the page being abandoned
            // BEFORE reopening the done-set (the done-set entries are
            // the armed-root ledger retire walks). hw4w1 flipped slot 1
            // across four same-tgid words every 5 s census round; each
            // flip left the old page W-cleared and the next legitimate
            // store to it faulted SEGV_ACCERR (hw4 t=116.349, foundation
            // tid 1470's musl release store on slot 2's abandoned page).
            fuxwatch_retire_page(s1);
            FUXWATCH_ADDR.store(word, Relaxed);
            reopen_done_set();
            arm_print(1, word);
        }
        return;
    }
    // Foreign tgid relative to slot 1: slot 2 is dedicated to the SECOND
    // distinct stalled tgid and stays sticky afterwards.
    let s2 = FUXWATCH2_ADDR.load(Relaxed);
    let s2_t = FUXWATCH2_TGID.load(Relaxed);
    if s2 == 0 {
        if FUXWATCH2_ADDR
            .compare_exchange(0, word, Relaxed, Relaxed)
            .is_ok()
        {
            FUXWATCH2_TGID.store(tgid, Relaxed);
            // neverword lane (nw1 conviction): the FIRST slot-2 latch
            // reopened the done-set with slot 1's page still W-cleared
            // in every mm the arm round had covered — those mms left
            // the ledger with present-RO PTEs, and the fault hook
            // (mm_armed now false) fell them to do_page_fault ->
            // SEGV_ACCERR. nw1 t=6.56-6.71: slot1 armed samgr
            // (0x3fbf13e6a8), the hilogd decode latched slot 2 on the
            // SAME word ~55 ms later, samgr tid 361's musl arena CAS
            // faulted on the abandoned page, the group died, init
            // rebooted (panic) at 10.1 s. Retire slot 1's page (walks
            // the ledger — MUST precede the reopen) so every armed mm
            // gets W back before the ledger is cleared.
            fuxwatch_retire_page(s1);
            reopen_done_set();
            arm_print(2, word);
        }
    } else if s2_t == tgid && (s2 & !0xfff) != (word & !0xfff) {
        fuxwatch_retire_page(s2);
        FUXWATCH2_ADDR.store(word, Relaxed);
        reopen_done_set();
        arm_print(2, word);
    }
}
