//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The process subsystem's code (seed-core-design §2c.3): everything of the
//! subsystem that is neither `unsafe.rs` nor `types.rs`, in one file.
//!
//! Sections follow the order of the old module declarations; each banner
//! names the topic file it was (`task` was task.rs) and the file's module
//! doc follows it as the section's header comment. Inside a section the
//! items keep their original order. Type definitions are in `types.rs`,
//! the audited unsafe edges in `unsafe.rs`; the unsafe that is still
//! scattered over the task_ref and process-root sections is collected in
//! pass 3.
//!
//! Eight of the ten merged files opened with `#![forbid(unsafe_code)]`. The
//! merged file does not carry it, because task_ref.rs and mod.rs hold the
//! unsafe just named; the attribute comes back in pass 3, when the
//! scattered unsafe has moved to `unsafe.rs`.
//!
//! The file's NAME overstates until the shrink stage has run: it holds all
//! of the subsystem's code under the hot-path name, not a verified set of
//! hot-path functions (§2c.3, user decision 2026-11-25).

use super::types::{
    BIRTH_CALLS_LEN,
    Cred,
    FreeStackEntry,
    FreedStackRec,
    INIT_CALLS_LEN,
    IpcNamespace,
    LAST_CALLS_LEN,
    MountEntry,
    MountNamespace,
    NetNamespace,
    NsFileContent,
    NsKind,
    NsRef,
    Pid,
    PidNamespace,
    PosixTimerState,
    RLIM_NLIMITS,
    SchedPolicy,
    StackCache,
    Task,
    TaskBorrow,
    TaskExt,
    TaskExtHandle,
    TaskRef,
    TaskState,
    UTS_LEN,
    UserNamespace,
    UtsNamespace,
    WaitQueueEntry,
    WaitQueueHead,
    WakeUpHint,
    ZeroEntry,
};
#[cfg(target_arch = "x86_64")]
use super::types::KstackBlocks;

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use core::ptr;
use crate::sync::types::Spinlock;
use crate::mm::types::AddressSpace;
use crate::fs::types::FdTable;
use crate::signal::{SigInfo, SigPending, Signal, SignalStruct};
use crate::config::TIME_SLICE_TICKS as DEFAULT_TIME_SLICE;
use alloc::boxed::Box;
use alloc::alloc::{alloc, dealloc};
use core::alloc::Layout;
use core::mem::offset_of;
use crate::list::ListHead;
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;
use crate::sync::hotpath;
/// Per-bucket chain head.  AtomicPtr would be ideal but we only have
/// core::sync::atomic in no_std, so use a Spinlock-protected array.
/// Actually, we do have AtomicPtr in core.  Let's use it.
use core::sync::atomic::AtomicPtr;
use alloc::vec::Vec;
use crate::errno;
use alloc::string::String;
use alloc::sync::Arc;
// ════════════════ process root ════════════════════════
// (was mod.rs: the module's own functions and its re-export of the api's
// kthread entry points; the topic modules and their re-exports are gone —
// every path now names types or hotpath)
// Process management module
//
// This module implements process management functionality.
// - `task`: Process control block (task_struct)
// - `fork`: Process creation
// - `wait`: Wait queues

// pub mod reboot; — moved to the process service crate (the file
// now physically lives in kernel/process/src/).
// pub mod fork; — moved to the process service crate (crate-flip
// rung: process); the crate's process module re-exports it.
// pub mod pid; — moved to the process service crate.
// pub mod exec; — moved to the process service crate; the
// STACKZERO ring statics the riscv64 fault path walks stay
// framework-side here (hot read, no hook row).
// pub mod kthread; — moved to the process service crate.
// The wrapper surface for sibling crates and framework callers
// (the hook-table indirection lives in process_hooks; these are the
// #[inline] entry points).
pub use crate::api::process::{kthread_run, kthread_run_on_cpu, kthread_should_stop};
// pub mod ptrace; — moved to the process service crate; the two
// entry points the trap files call by name stay core-side:
// pub mod coredump; — moved to the process service crate.
// The flip seam (ProcessHooks, KthreadHooks, PidHooks, ExitHooks) is
// crate::api::process.

// ==================== Arch ThreadStruct bridges ====================
// Small cfg helpers so generic process code never touches arch-specific
// ThreadStruct field layouts directly.

/// Zero the kernel-side callee-saved registers of a fresh task (the

/// Seed a fresh task's context-switch state: the first switch-in
/// "returns" into `entry` with `sp` as the kernel stack pointer.
pub fn thread_set_entry(
    thread: &mut crate::arch::thread::ThreadStruct,
    entry: u64,
    sp: u64,
) {
    thread.sp = sp;
    #[cfg(feature = "riscv64")]
    {
        thread.ra = entry;
    }
    #[cfg(feature = "aarch64")]
    {
        // Same shape as riscv64: __switch_to returns into thread.ra with
        // sp = thread.sp. The fork child's ra = ret_from_fork, whose
        // first switch-in runs the user-exit loop into the seeded
        // pt_regs frame (child erets to EL0 with x0 = 0).
        thread.ra = entry;
    }
    #[cfg(feature = "x86_64")]
    {
        thread.callee.ret_addr = entry;
    }
}

/// Set the task's user TLS pointer (riscv64: tp register in pt_regs;
/// x86_64: FS base restored on switch-in; aarch64: TPIDR_EL0 saved in

/// Get current process ID
pub fn current_pid() -> u32 {
    crate::sched::hotpath::get_current_pid()
}

/// Get current parent process ID
pub fn current_ppid() -> u32 {
    crate::sched::hotpath::get_current_ppid()
}

/// Get current process group ID
pub fn current_pgid() -> u32 {
    // SAFETY: sched::current() returns a valid pointer to the currently running task.
    crate::sched::hotpath::current().map_or(0, |t| unsafe { (*t).pgid() })
}

/// Get current task reference
///
/// Returns None if no current task is set (e.g., during early boot)
pub fn current_task() -> Option<&'static mut Task> {
    crate::sched::hotpath::current()
}

/// Find task by PID
///
/// Uses the PID hash table for O(log N) lookup.
/// Works for all task states (running, sleeping, zombie).
pub fn find_task_by_pid(pid: u32) -> Option<&'static mut Task> {
    let ptr = self::pid_hash_lookup(pid);
    if ptr.is_null() {
        None
    } else {
        // SAFETY: pid_hash_lookup returns a valid Task pointer from the PID hash table.
        // The pointer remains valid as long as the task is alive (not reaped).
        Some(unsafe { &mut *ptr })
    }
}


// ════════════════ task ════════════════════════════════
// (was task.rs)
// Task Control Block

/// Kernel stack size - from config
///
/// RISC-V typically uses 16KB kernel stack, but we increase to 32KB
/// because some operations (like FdTable creation) need larger stack space
pub(crate) const KERNEL_STACK_SIZE: usize = crate::config::KERNEL_STACK_SIZE;

// ==================== Stack Cache Implementation ====================

/// Global lock protecting the process tree (children/sibling lists, parent pointers).
///
/// Must be held when modifying or traversing the children list of any task.
/// Do NOT hold across schedule() calls.
pub static PROCESS_TREE_LOCK: Spinlock<()> = Spinlock::new(());

impl StackCache {
    const fn new() -> Self {
        Self {
            count: 0,
            head: core::ptr::null_mut(),
        }
    }

    /// Maximum number of cached stacks
    const MAX_CACHED_STACKS: usize = 64;

    /// Pop a stack from cache
    fn pop(&mut self) -> Option<*mut u8> {
        if self.head.is_null() {
            return None;
        }
        // (canary written at the very bottom AFTER zeroing in stack_cache_alloc)
        // head is non-null (checked above) and was pushed by push() under
        // the STACK_CACHE lock (INV-PROC-KSTACK-1).
        let entry = self.head;
        self.head = crate::process::r#unsafe::freelist_next(entry as *mut u8) as *mut FreeStackEntry;
        self.count -= 1;
        Some(entry as *mut u8)
    }

    /// Push a stack to cache
    /// R18-2: stack-overflow canary — written below the usable stack at
    /// cache-pop time, verified at push time. A smashed canary means some
    /// task's kernel stack ran past its 32KB into the adjacent heap page
    /// (the suspected zeroing writer).
    const STACK_CANARY: u64 = 0xCAFE_F00D_DEAD_BEEF;

    fn push(&mut self, stack_bottom: *mut u8) {
        if self.count >= Self::MAX_CACHED_STACKS {
            // Leg-17: on x86_64 route through the packed slab (slot map
            // + empty-block reclaim); the plain GlobalAlloc dealloc is
            // the non-slab path (every other arch) and the fallback for
            // exhaustion-allocated stacks. We hold STACK_CACHE;
            // KSTACK_BLOCKS nests BELOW it (leaf) and the buddy free
            // inside runs outside KSTACK_BLOCKS — same STACK_CACHE ->
            // BUDDY nesting the old dealloc had.
            #[cfg(target_arch = "x86_64")]
            {
                if kstack_slab_free(stack_bottom) {
                    return;
                }
            }
            // stack_bottom was allocated by stack_cache_alloc() with the
            // same layout under STACK_CACHE_LOCK (INV-PROC-KSTACK-2).
            let layout = Layout::from_size_align(KERNEL_STACK_SIZE, 16)
                .unwrap_or_else(|_| Layout::new::<[u8; KERNEL_STACK_SIZE]>());
            crate::process::r#unsafe::heap_dealloc(stack_bottom, layout);
            return;
        }
        // stack_bottom was returned by alloc(); the unused bottom region
        // carries the free-list linkage (INV-PROC-KSTACK-1).
        let entry = stack_bottom as *mut FreeStackEntry;
        crate::process::r#unsafe::freelist_link(
            entry as *mut u8,
            self.head as *mut u8,
        );
        self.head = entry;
        self.count += 1;
    }
}

// Send impl lives in unsafe.rs (task.rs carries zero unsafe).

/// Global stack cache instance (wrapped in Spinlock, no separate lock needed).
static STACK_CACHE: Spinlock<StackCache> = Spinlock::new(StackCache::new());

// ===== Leg-17 packed kstack slab — x86_64 ONLY =====
//
// The buddy only serves power-of-two blocks, so a non-power-of-two
// stack request consumes a whole larger block. KERNEL_STACK_SIZE on
// x86_64 is 128KB (see Kernel.toml kernel_stack_size_x86_64): the
// interim 80KB carve (3 x 80KB = 240KB per 256KB order-6 block, ~15KB
// margin) held for OH boots but NOT under LTP — the depth-capped (40)
// recursive symlink resolution in fs/vfs.rs costs ~2240B of stack per
// nested hop ≈ 90KB for a maximal chain, past the 80KB bottom before
// the depth counter can return ELOOP (nftw01 chunk@700 boot panic,
// 4/4 deterministic, 2026-11-02; rsp 19KB below the bottom, frames
// torn through a recycled adjacent slot, ret to rip=0). 128KB restores
// the riscv64-parity floor this file's history already documents for
// deep-nesting kernels.
//
// At 128KB the carver serves TWO slots per 256KB order-6 block with
// zero tail waste, and the buddy fallback is one perfectly-packed
// order-5 block (the slab is then pure bookkeeping; kept so exhausted
// order-6 blocks still fall back cleanly).
//
// Slot state lives in a side table under KSTACK_BLOCKS — the stack
// pages themselves stay pure (R18-2 canary still sits at each SLOT
// bottom). Lock order: KSTACK_BLOCKS is a leaf; the buddy block
// alloc/free runs OUTSIDE it (take the lock, pick/insert, drop, then
// touch the buddy). Fully-empty blocks return to the buddy.
#[cfg(target_arch = "x86_64")]
const KSTACK_BLOCK: usize = 262144; // order-6, 64 pages
#[cfg(target_arch = "x86_64")]
const KSTACK_SLOTS: usize = 2;
// The slot carve must fit the block (compile-time guard against a
// future kernel_stack_size_x86_64 edit, e.g. 96KB would need 288KB).
// compat-r3: 128KB stacks again (the 80KB/3-slot packing's margin was
// an OH-load figure; the LTP class overflowed it — nftw01's wild-jump
// boot panic + the nice=-120 corruption signature, see Kernel.toml).
// Two 128KB slots per 256KB block: exact fit, zero carve waste.
#[cfg(target_arch = "x86_64")]
const _: () = assert!(KSTACK_SLOTS * KERNEL_STACK_SIZE <= KSTACK_BLOCK);

// Send impl lives in unsafe.rs (task.rs carries zero unsafe).

#[cfg(target_arch = "x86_64")]
static KSTACK_BLOCKS: Spinlock<KstackBlocks> = Spinlock::new(KstackBlocks {
    bases: alloc::vec::Vec::new(),
    maps: alloc::vec::Vec::new(),
});

// P0 exit-reap (2026-11-23): KSTACK_BLOCKS and STACK_CACHE are taken
// from TIMER-IRQ context (the async reap drainer's burn:
// reap_drain_tick -> reap_storage_free -> free_kernel_stack ->
// stack_cache_free -> kstack_slab_free). A lock reachable from IRQ
// context must be irqsave on EVERY taker: a plain lock() held by a
// task-context fork (kstack_slab_alloc under stack_cache_alloc) and
// re-taken by the same CPU's tick drainer is a same-CPU recursion
// deadlock — the holder is suspended by the very interrupt that now
// spins on the lock (measured on the P4 storm, x86/KVM: pid 1 frozen
// in kstack_slab_alloc, TIMER-SOFTIRQ-STALL age climbing past 190 s,
// the sibling CPU alive; the drainer's burn path had NEVER run before
// the retire-site fix landed — 0 completions — so this discipline
// breach sat latent under the RTS-DEFER wedge). The leaf allocators
// were already irqsafe (buddy lock_irqsave, small_alloc class locks);
// these two intermediate locks are the whole gap.

/// Carve one stack out of a packed block (or fall back to a whole
/// order-5 buddy block when the slab cannot grow — keeps booting on
/// heap exhaustion instead of dying).
#[cfg(target_arch = "x86_64")]
fn kstack_slab_alloc() -> *mut u8 {
    // Fast path: existing block with a free slot.
    {
        let mut g = KSTACK_BLOCKS.lock_irqsave();
        for bi in 0..g.maps.len() {
            if g.maps[bi] == 0b111 {
                continue;
            }
            for s in 0..KSTACK_SLOTS {
                if g.maps[bi] & (1 << s) == 0 {
                    g.maps[bi] |= 1 << s;
                    // Block base is a live allocation of KSTACK_BLOCK
                    // bytes; slot s fits (const assert above).
                    return crate::process::r#unsafe::slot_ptr(g.bases[bi], s * KERNEL_STACK_SIZE);
                }
            }
        }
    }
    // Slow path: new order-6 block from the buddy (OUTSIDE the slab
    // lock — no lock nesting with BUDDY).
    let block = crate::process::r#unsafe::heap_alloc(
        Layout::from_size_align(KSTACK_BLOCK, 4096)
            .unwrap_or(Layout::new::<[u8; KSTACK_BLOCK]>()),
    );
    if block.is_null() {
        return core::ptr::null_mut();
    }
    {
        let mut g = KSTACK_BLOCKS.lock_irqsave();
        g.bases.push(block);
        g.maps.push(0b001); // slot 0 taken by this allocation
    }
    block
}

/// Return a slot to its block; a fully-empty block goes back to the
/// buddy. Returns false when the address is not a slab slot (the
/// fallback path allocated it as a plain order-5 block — caller frees
/// it through the normal GlobalAlloc path).
#[cfg(target_arch = "x86_64")]
fn kstack_slab_free(bottom: *mut u8) -> bool {
    let addr = bottom as usize;
    let mut free_block: *mut u8 = core::ptr::null_mut();
    {
        let mut g = KSTACK_BLOCKS.lock_irqsave();
        let mut bi = 0usize;
        let mut found = false;
        while bi < g.bases.len() {
            let base = g.bases[bi] as usize;
            if addr >= base && addr < base + KSTACK_BLOCK {
                found = true;
                let off = addr - base;
                if off % KERNEL_STACK_SIZE != 0 {
                    return true; // foreign pointer inside a slab block:
                    // consume it (do NOT touch the slot map)
                }
                let s = off / KERNEL_STACK_SIZE;
                if s >= KSTACK_SLOTS || g.maps[bi] & (1 << s) == 0 {
                    return true; // double free / not allocated: consume
                }
                g.maps[bi] &= !(1 << s);
                if g.maps[bi] == 0 {
                    free_block = g.bases[bi];
                    g.bases.swap_remove(bi);
                    g.maps.swap_remove(bi);
                }
                break;
            }
            bi += 1;
        }
        if !found {
            return false; // not our memory: plain buddy allocation
        }
    }
    if !free_block.is_null() {
        // Buddy free OUTSIDE the slab lock; the block was allocated by
        // kstack_slab_alloc with this layout and is now fully empty.
        crate::process::r#unsafe::heap_dealloc(
            free_block,
            Layout::from_size_align(KSTACK_BLOCK, 4096)
                .unwrap_or(Layout::new::<[u8; KSTACK_BLOCK]>()),
        );
    }
    true
}

/// Packed-slab state for the SC-ANAT beat line (rux:schb (retired debug=schb)):
/// (blocks, live slots, fallback note). x86_64 only — every other arch
/// keeps the plain power-of-two buddy allocation and reports zeros.
#[cfg(target_arch = "x86_64")]
pub fn kstack_slab_stats() -> (u64, u64, u64) {
    let g = KSTACK_BLOCKS.lock_irqsave();
    let mut live: u64 = 0;
    for bm in g.maps.iter() {
        live += bm.count_ones() as u64;
    }
    (g.bases.len() as u64, live, 0)
}

/// Non-x86_64 shim: no packed slab — the beat line stays format-stable
/// across arches (kb=0 ks=0 reads as "no slab", which is the truth).
#[cfg(not(target_arch = "x86_64"))]
pub fn kstack_slab_stats() -> (u64, u64, u64) {
    (0, 0, 0)
}

// Leg-16 kernel-heap anatomy (rux:schb (retired debug=schb)): live kernel-stack count and the
// boot-max stack high-water in bytes. The high-water probe ZEROES each
// fresh stack at alloc (outside every lock) and scans for the first
// nonzero word from the bottom at free time — the depth distribution is
// the evidence for any KERNEL_STACK_SIZE reduction, which at pidmax ~4800
// is 128KB x ~2860 live tasks = ~358MB of the ~900MB heap floor (the
// single largest per-task item; see SC-SESSIONS leg-15 close-out).
static KSTACK_LIVE: AtomicU64 = AtomicU64::new(0);
static KSTACK_HW: AtomicU64 = AtomicU64::new(0);

/// Leg-16 anatomy: (live kernel-stack count, boot-max high-water bytes).
/// Both stay 0 unless rux:schb (retired debug=schb) armed the probe at alloc time.
pub fn kstack_anat() -> (u64, u64) {
    (KSTACK_LIVE.load(Ordering::Relaxed), KSTACK_HW.load(Ordering::Relaxed))
}

/// Plain buddy allocation of one kernel stack (the pre-slab path).
/// On x86_64 this is only the packed-slab exhaustion fallback (the
/// 128KB request is one perfectly-packed order-5 block); elsewhere it
/// is THE allocation path.
fn plain_kstack_alloc() -> *mut u8 {
    let layout = Layout::from_size_align(KERNEL_STACK_SIZE, 16)
        .unwrap_or_else(|_| Layout::new::<[u8; KERNEL_STACK_SIZE]>());
    crate::process::r#unsafe::heap_alloc(layout)
}

/// Allocate a kernel stack (with caching)
fn stack_cache_alloc() -> *mut u8 {
    // Reuse is DISABLED: freed stacks re-enter the cache while the dying
    // task can still be executing its final sret/exit path on that stack
    // (reaper/sweep races), and a pop hands the SAME memory to a new task —
    // merging two tasks' kernel contexts. Fingerprint (reproduced twice,
    // incl. dash-parallel): user-mode SIGSEGV with epc/ra pointing into
    // kernel uaccess code (__copy_to_user/__clear_user fixup stubs) — a
    // user sret restored a pt_regs that was a KERNEL-mode fault frame
    // written by the new owner of the reused stack. Cost of disabling:
    // bounded memory (cache grows with peak task churn); cost of enabling
    // with the early-free unfixed: cross-task context corruption.
    //
    // With reuse disabled the lock guards nothing on this path, so the
    // heap allocation (BUDDY nest) and the leg-16 probe zeroing run
    // OUTSIDE it — no 128KB memset under a spinlock.
    {
        let mut cache = STACK_CACHE.lock_irqsave();
        let _ = &mut cache;
    }
    // Leg-17: on x86_64 stacks come from the packed slab first (two
    // 128KB slots per 256KB order-6 block — see the slab block comment);
    // a whole order-5 buddy block is only the exhaustion fallback.
    // Every other arch keeps the plain power-of-two allocation, where
    // 128KB is already one perfectly-packed order-5 block.
    #[cfg(target_arch = "x86_64")]
    let bottom = {
        let mut b = kstack_slab_alloc();
        if b.is_null() {
            b = plain_kstack_alloc();
        }
        b
    };
    #[cfg(not(target_arch = "x86_64"))]
    let bottom = plain_kstack_alloc();
    if !bottom.is_null() {
        // SCRIBBLE TRAP scan side (oh-launch round 9): if the scribble
        // arm painted this stack at free time, any NON-poison word now
        // was written AFTER the free — by the early-free dying task or
        // by the recycled-frame writer. Report the first hits with the
        // previous owner (the FREED_STACKS ring names it). The scan runs
        // BEFORE the leg-16 zeroing and the canary write; armed boots
        // only; cold allocation path.
        let scribble_scan_known = crate::trace::EVENT_SCRIBBLE.armed()
            && freed_stack_containing(bottom as usize + KERNEL_STACK_SIZE / 2).is_some();
        if scribble_scan_known {
            // Only slots the FREED_STACKS ring knows were painted — a
            // fresh slab carve was never painted and its stale content
            // is not evidence (fq1's 16 first-boot hits were all
            // prev-owner-not-found false positives).
            let base = bottom as usize;
            let mut hits = 0u32;
            let mut first_off = 0usize;
            let mut first_val = 0u64;
            // The whole stack is KERNEL_STACK_SIZE bytes of readable
            // memory owned by this allocation. Word 0 is the canary slot
            // — never painted (R87 paint skips it so stack_cache_free's
            // check stays meaningful) — skip it.
            {
                let words = KERNEL_STACK_SIZE / 8;
                for i in 1..words {
                    let v = crate::process::r#unsafe::read_volatile_u64((base + i * 8) as *const u64);
                    if v != 0xA5A5_A5A5_A5A5_A5A5 {
                        if hits == 0 {
                            first_off = i * 8;
                            first_val = v;
                        }
                        hits += 1;
                        if hits >= 8 {
                            break;
                        }
                    }
                }
            }
            if hits > 0 {
                let prev = freed_stack_containing(base + KERNEL_STACK_SIZE / 2)
                    .map(|(_, pid, age)| (pid, age))
                    .unwrap_or((0, 0));
                crate::pr_err!(
                    "KSTACK-POSTFREE-WRITE bottom={:#x} first_off={:#x} val={:#x} hits={} prev_owner_pid={} freed_age={}j",
                    base,
                    first_off,
                    first_val,
                    hits,
                    prev.0,
                    prev.1
                );
            }
        }
        if crate::trace::EVENT_SCHB.armed()
            && crate::trace::EVENT_KSPROBE.armed()
        {
            // Fresh stack is visible to nobody else yet: zero it so the
            // free-side scan can measure the true high-water, and count it.
            // ~3 spawns/s x 128KB of memset across the OH boot — negligible
            // next to the fork itself.
            crate::process::r#unsafe::zero_bytes(bottom, KERNEL_STACK_SIZE);
            KSTACK_LIVE.fetch_add(1, Ordering::Relaxed);
        }
        // fresh alloc also gets a canary
        crate::process::r#unsafe::write_volatile_u64(
            bottom as *mut u64,
            StackCache::STACK_CANARY,
        );
    }
    bottom
}

static FREED_STACKS: Spinlock<([FreedStackRec; 32], usize)> =
    Spinlock::new(([const { FreedStackRec { bottom: 0, top: 0, pid: 0, free_jiffies: 0 } }; 32], 0));

/// Record a stack free (call before the memory is reusable).
pub fn note_kstack_freed(bottom: usize, top: usize, pid: u32) {
    let mut g = FREED_STACKS.lock();
    let idx = g.1;
    g.0[idx].bottom = bottom;
    g.0[idx].top = top;
    g.0[idx].pid = pid;
    g.0[idx].free_jiffies = crate::timer::get_jiffies();
    g.1 = (idx + 1) % 32;
}

/// Read-side lookup for fault diagnostics: the most recent record whose
/// range contains `sp`, if any.
pub fn freed_stack_containing(sp: usize) -> Option<(usize, u32, u64)> {
    let g = FREED_STACKS.lock();
    let now = crate::timer::get_jiffies();
    let mut best: Option<(u64, usize, u32, u64)> = None;
    for r in g.0.iter() {
        if r.bottom != 0 && sp >= r.bottom && sp < r.top {
            match best {
                Some((j, ..)) if j >= r.free_jiffies => {}
                _ => best = Some((r.free_jiffies, r.top, r.pid, now.wrapping_sub(r.free_jiffies))),
            }
        }
    }
    best.map(|(_, top, pid, age)| (top, pid, age))
}

/// Free a kernel stack (with caching)
fn stack_cache_free(stack_bottom: *mut u8) {
    // R18-2: verify the canary before returning the stack to the cache —
    // a smashed value is direct proof the task ran past its stack into
    // the adjacent heap page (stack_bottom is a valid stack base from
    // alloc; the first 8 bytes are in bounds).
    {
        let v = crate::process::r#unsafe::read_volatile_u64(stack_bottom as *const u64);
        if v != StackCache::STACK_CANARY {
            use crate::console::putchar;
            const MSG: &[u8] = b"STACK-OVERFLOW canary=0x";
            for &b in MSG { putchar(b); }
            let mut sh = 64;
            while sh > 0 {
                sh -= 4;
                let nb = ((v >> sh) & 0xF) as u8;
                putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
            }
            const MB: &[u8] = b" bottom=0x";
            for &b in MB { putchar(b); }
            {
                let a2 = stack_bottom as usize;
                let mut sh3 = 64;
                while sh3 > 0 { sh3 -= 4; let nb = ((a2 >> sh3) & 0xF) as u8; putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 }); }
            }
            const M2: &[u8] = b" pid=";
            for &b in M2 { putchar(b); }
            if let Some(t) = crate::sched::hotpath::current() {
                let pid = t.pid();
                let mut vv = pid as usize;
                if vv == 0 { putchar(b'0'); }
                let mut dd = [0u8; 10]; let mut kk = 0;
                while vv > 0 { dd[kk] = b'0' + (vv % 10) as u8; kk += 1; vv /= 10; }
                while kk > 0 { kk -= 1; putchar(dd[kk]); }
                // frame walk naming the deepest live frames
                crate::debug::backtrace::walk_stack_trace(&mut |pc, _fp| {
                    let _ = pc;
                });
            }
            putchar(b'\n');
            // Identify the overflower: dump the bottom 256 bytes of the
            // stack above the canary — return addresses of the deepest
            // frames survive there.
            {
                const M3: &[u8] = b" tail:";
                for &b in M3 { putchar(b); }
                let base = stack_bottom as usize;
                for off in (8..256).step_by(8) {
                    let ra = crate::process::r#unsafe::read_volatile_u64((base + off) as *const u64);
                    if ra >= 0xffffffff_80000000u64 && ra <= 0xffff_ffff_ffff_f000u64 {
                        putchar(b' ');
                        let mut sh2 = 64;
                        while sh2 > 0 {
                            sh2 -= 4;
                            let nb = ((ra >> sh2) & 0xF) as u8;
                            putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
                        }
                    }
                }
                putchar(b'\n');
            }
        }
    }
    // Leg-16 anatomy (rux:schb (retired debug=schb),ksprobe): the high-water scan + record
    // stash lives in free_kernel_stack (needs self for comm/pid); this
    // side only keeps the live count honest for the SC-ANAT kN column.
    if crate::trace::EVENT_SCHB.armed()
        && crate::trace::EVENT_KSPROBE.armed()
    {
        KSTACK_LIVE.fetch_sub(1, Ordering::Relaxed);
    }
    let mut cache = STACK_CACHE.lock_irqsave();
    // SAFETY: stack_bottom was allocated by stack_cache_alloc().
    cache.push(stack_bottom);
}

impl TaskState {
    /// Runnable state (TASK_RUNNING)
    /// Process is running on CPU or waiting in run queue
    pub const RUNNING: u32 = 0x00000000;

    /// Interruptible sleep (TASK_INTERRUPTIBLE)
    /// Process is waiting for an event, can be woken by signal
    pub const INTERRUPTIBLE: u32 = 0x00000001;

    /// Uninterruptible sleep (TASK_UNINTERRUPTIBLE)
    /// Process is waiting for an event, cannot be woken by signal
    pub const UNINTERRUPTIBLE: u32 = 0x00000002;

    /// Stopped state (__TASK_STOPPED)
    /// Process stopped by signal (SIGSTOP, SIGTSTP, etc.)
    pub const STOPPED: u32 = 0x00000004;

    /// Traced state (__TASK_TRACED)
    /// Process is being traced by ptrace
    pub const TRACED: u32 = 0x00000008;

    /// Exit zombie (EXIT_ZOMBIE)
    /// Process has exited but parent hasn't waited yet
    pub const ZOMBIE: u32 = 0x00000020;

    /// Exit dead (EXIT_DEAD)
    /// Process final state, will be reclaimed
    pub const DEAD: u32 = 0x00000010;

    /// Newly constructed, not yet on a run queue (Linux TASK_NEW)
    ///
    /// R52 (fork enqueue gap): `new_task_at` writes this INSTEAD of
    /// RUNNING. Between construction and `enqueue_task` the task is
    /// already visible in the pid hash with a valid pid — previously it
    /// also carried state=RUNNING there, so ANY concurrent wake routed
    /// through Task::wake_up / signal_wake_up_state reached the R46
    /// "RUNNING-phantom" heal and LINKED THE HALF-BUILT CHILD (no
    /// thread.sp/ra yet, no mm); a CPU then picked it and context
    /// switched onto constructor values — the R33/R48 wedge family —
    /// while do_clone's own enqueue was silently eaten by the class
    /// on_rq guard (result ignored at fork.rs:449). TASK_NEW is refused
    /// by every wake route (not sleeping, not STOPPED, not RUNNING),
    /// matching Linux: a half-built task cannot be woken, killed, or
    /// OOM-selected. The NEW→RUNNING transition happens only inside
    /// `enqueue_task_locked`, atomically with the class insert under
    /// the GRQ lock.
    pub const TASK_NEW: u32 = 0x00000080;

    /// Create new state
    #[inline]
    pub const fn new(bits: u32) -> Self {
        TaskState(bits)
    }

    /// Get bit value
    #[inline]
    pub fn bits(&self) -> u32 {
        self.0
    }

    /// Check if contains specified flag
    #[inline]
    pub fn contains(&self, flag: u32) -> bool {
        (self.0 & flag) != 0
    }

    /// Check if running
    #[inline]
    pub fn is_running(&self) -> bool {
        self.0 == Self::RUNNING
    }

    /// Check if sleeping (interruptible or uninterruptible)
    #[inline]
    pub fn is_sleeping(&self) -> bool {
        self.contains(Self::INTERRUPTIBLE) || self.contains(Self::UNINTERRUPTIBLE)
    }

    /// Check if exited (zombie or dead)
    #[inline]
    pub fn is_dead(&self) -> bool {
        self.contains(Self::ZOMBIE) || self.contains(Self::DEAD)
    }

    /// Check if can be woken by signal
    #[inline]
    pub fn is_interruptible(&self) -> bool {
        self.contains(Self::INTERRUPTIBLE)
    }
}

impl Default for TaskState {
    fn default() -> Self {
        TaskState::new(TaskState::RUNNING)
    }
}


/// SECBIT_KEEP_CAPS (bit 4) in the prctl securebits encoding.
pub const SECBIT_KEEP_CAPS: u32 = 1 << 4;
/// SECBIT_NO_SETUID_FIXUP (bit 2) and its lock (bit 3) in the prctl
/// securebits encoding (linux/securebits.h: the prctl argument is
/// issecure_mask(X) = 1 << X, with SECURE_NOROOT=0, SECURE_NO_SETUID_FIXUP=2,
/// SECURE_KEEP_CAPS=4 — NOT the raw X values).
pub const SECBIT_NO_SETUID_FIXUP: u32 = 1 << 2;
pub const SECBIT_NO_SETUID_FIXUP_LOCKED: u32 = 1 << 3;

impl Cred {
    /// Create credential for init (PID 0) / kernel threads.
    /// All capabilities are granted (root-equivalent).
    pub fn new_init() -> Self {
        Self {
            uid: 0, gid: 0,
            euid: 0, egid: 0,
            suid: 0, sgid: 0,
            fsuid: 0, fsgid: 0,
            groups: alloc::vec::Vec::new(),
            cap_inheritable: crate::security::capability::Cap::EMPTY,
            cap_permitted:   crate::security::capability::Cap::FULL,
            cap_effective:   crate::security::capability::Cap::FULL,
            cap_bounding:    crate::security::capability::Cap::FULL,
            cap_ambient:     crate::security::capability::Cap::EMPTY,
            keepcaps: false,
            securebits: 0,
            thread_keyring: None,
            process_keyring: None,
            session_keyring: None,
            reqkey_defl: 0,
        }
    }

    /// True when the setuid-away-from-root capability fixup is suppressed —
    /// any of PR_SET_KEEPCAPS, SECBIT_KEEP_CAPS (mirrored into `keepcaps`
    /// by prctl), or SECBIT_NO_SETUID_FIXUP(+_LOCKED). Linux's
    /// cap_emulate_setxuid skips the permitted/effective clearing when
    /// issecure(SECURE_NO_SETUID_FIXUP); OH's init programs exactly that
    /// bit (KeepCapability → prctl(PR_SET_SECUREBITS, SECBIT_NO_SETUID_FIXUP
    /// | _LOCKED)) on every setuid service before dropping to the service
    /// uid and then capset()ing the service's configured capability list
    /// (installs.cfg: DAC_OVERRIDE/CHOWN/FOWNER/SYS_ADMIN). When only
    /// PR_SET_KEEPCAPS is honored, setuid clears the sets and the capset
    /// gets EPERM — init kills the child with "failed to set
    /// capabilities" (exit 255), which crash-looped the `installs`
    /// bundle-installer SA (SA 511) and left BMS with no installed
    /// bundles (bm dump / aa start both fail downstream).
    pub fn keeps_caps_on_setuid(&self) -> bool {
        self.keepcaps
            || (self.securebits
                & (SECBIT_NO_SETUID_FIXUP | SECBIT_NO_SETUID_FIXUP_LOCKED))
                != 0
    }

    /// Create credential with no capabilities (for user processes).
    pub fn new_user(uid: u32, gid: u32) -> Self {
        Self {
            uid, gid,
            euid: uid, egid: gid,
            suid: uid, sgid: gid,
            fsuid: uid, fsgid: gid,
            groups: alloc::vec::Vec::new(),
            cap_inheritable: crate::security::capability::Cap::EMPTY,
            cap_permitted:   crate::security::capability::Cap::EMPTY,
            cap_effective:   crate::security::capability::Cap::EMPTY,
            cap_bounding:    crate::security::capability::Cap::FULL,
            cap_ambient:     crate::security::capability::Cap::EMPTY,
            keepcaps: false,
            securebits: 0,
            thread_keyring: None,
            process_keyring: None,
            session_keyring: None,
            reqkey_defl: 0,
        }
    }

    /// Check if a gid matches egid or any supplementary group
    pub fn in_group(&self, gid: u32) -> bool {
        if self.egid == gid {
            return true;
        }
        self.groups.iter().any(|&g| g == gid)
    }
}

// ==================== thread_info style flags ====================

/// TIF_SIGPENDING - has pending signal
pub const TIF_SIGPENDING: u32 = 0;
/// TIF_NEED_RESCHED - needs rescheduling
pub const TIF_NEED_RESCHED: u32 = 1;
/// TIF_NOTIFY_RESUME - notify before returning to user mode
pub const TIF_NOTIFY_RESUME: u32 = 2;
/// TIF_UPROBE - uprobe pending
pub const TIF_UPROBE: u32 = 3;
/// TIF_MEMDIE - exiting (out of memory)
pub const TIF_MEMDIE: u32 = 4;

/// R56-heal pick-state machine values (Task::ti_pick_state).
/// PICKED: published by mark_picked_on_cpu under the GRQ.
/// LIVE: affirmed by __switch_to's CAS(0 -> 1) right before the register
/// restore (or by the next == prev fast-path repick publish).
/// REVOKED: the harvester's CAS(0 -> 2) — the picker's switch-in aborts.
pub const PICK_PICKED: u32 = 0;
pub const PICK_LIVE: u32 = 1;
pub const PICK_REVOKED: u32 = 2;

/// R56-heal: pin the layout facts the aligned ownership-qword stores
/// (publish_pick_ownership / release_pick_ownership, and the switch asm)
/// rely on, at compile time — ti_on_cpu is 8-byte aligned and
/// running_on_cpu sits at +4 inside the same qword.
const _: () = {
    assert!(core::mem::offset_of!(Task, ti_on_cpu) % 8 == 0);
    assert!(
        core::mem::offset_of!(Task, running_on_cpu)
            == core::mem::offset_of!(Task, ti_on_cpu) + 4
    );
    assert!(core::mem::offset_of!(Task, ti_pick_state) % 4 == 0);
};

/// RLIMIT resource indices (asm-generic, same as Linux uapi).
#[allow(dead_code)]
pub mod rlimit_res {
    pub const CPU: usize = 0;
    pub const FSIZE: usize = 1;
    pub const DATA: usize = 2;
    pub const STACK: usize = 3;
    pub const CORE: usize = 4;
    pub const RSS: usize = 5;
    pub const NPROC: usize = 6;
    pub const NOFILE: usize = 7;
    pub const MEMLOCK: usize = 8;
    pub const AS: usize = 9;
    pub const LOCKS: usize = 10;
    pub const SIGPENDING: usize = 11;
    pub const MSGQUEUE: usize = 12;
    pub const NICE: usize = 13;
    pub const RTPRIO: usize = 14;
    pub const RLIMIT_RTTIME: usize = 15;
}

/// Kernel default limits (cur, max):
/// - NOFILE 1024/4096 (soft default / table size, fs::file::MAX_FDS)
/// - STACK 8MB soft / RLIM_INFINITY hard
/// - CORE 0 (Ubuntu/systemd ship RLIMIT_CORE=0; an unbounded soft limit
///   made every user crash stream a full core dump — under TCG the dump
///   I/O wedged the machine on real workloads)
/// - everything else RLIM_INFINITY (u64::MAX) like Linux's init_cred.
pub fn default_rlimits() -> [(u64, u64); RLIM_NLIMITS] {
    let mut t = [(u64::MAX, u64::MAX); RLIM_NLIMITS];
    t[rlimit_res::NOFILE] = (1024, 4096);
    t[rlimit_res::STACK] = (8 * 1024 * 1024, u64::MAX);
    t[rlimit_res::CORE] = (0, u64::MAX);
    t
}

impl Clone for PosixTimerState {
    fn clone(&self) -> Self {
        Self {
            kernel_timer_id: self.kernel_timer_id,
            clock_id: self.clock_id,
            interval_jiffies: self.interval_jiffies,
            sigev_signo: self.sigev_signo,
            sigev_notify: self.sigev_notify,
            overrun_count: self.overrun_count,
            user_timer_id: self.user_timer_id,
            cputime_deadline_ns: core::sync::atomic::AtomicU64::new(
                self.cputime_deadline_ns.load(core::sync::atomic::Ordering::Relaxed),
            ),
            cputime_interval_ns: core::sync::atomic::AtomicU64::new(
                self.cputime_interval_ns.load(core::sync::atomic::Ordering::Relaxed),
            ),
            wall_deadline_ticks: core::sync::atomic::AtomicU64::new(
                self.wall_deadline_ticks.load(core::sync::atomic::Ordering::Relaxed),
            ),
            interval_ns: self.interval_ns,
        }
    }
}

impl Task {
    /// Create new task
    ///
    pub fn new(pid: Pid, policy: SchedPolicy) -> Self {
        // PRIO_TO_PRIO: static_prio 120 -> prio 120
        let static_prio = 120; // DEFAULT_PRIO
        let normal_prio = static_prio; // For SCHED_NORMAL, normal_prio == static_prio
        let prio = normal_prio;

        // Idle task doesn't need file descriptor table and signal handling
        // Temporarily disable FdTable and Signal creation to avoid heap allocation issues
        let (fdtable, signal) = (None, None);

        let state = AtomicU32::new(TaskState::RUNNING);
        let pending = SigPending::new();
        let sigstack = crate::signal::SignalStack::new();

        let mut task = Self {
            // thread_info style fields
            ti_flags: AtomicU32::new(0),
            ti_preempt_count: core::sync::atomic::AtomicI32::new(0),
            ti_pc_zero_jif: AtomicU64::new(0),
            ti_pc_sirq_present_run: AtomicU64::new(0),
            ti_pc_hirq_present_run: AtomicU64::new(0),
            ti_pc_pirq_present_run: AtomicU64::new(0),
            ti_kernel_sp: core::sync::atomic::AtomicU64::new(0),
            ti_user_sp: core::sync::atomic::AtomicU64::new(0),
            ti_cpu: core::sync::atomic::AtomicI32::new(-1),
            cpus_allowed: core::sync::atomic::AtomicU32::new(Self::default_cpus_allowed()),
            hung_task_since: core::sync::atomic::AtomicU64::new(0),
            wq_regs: core::sync::atomic::AtomicU32::new(0),
            timed_sleep_deadline: core::sync::atomic::AtomicU64::new(0),
            timed_sleep_kind: core::sync::atomic::AtomicU32::new(0),
            timed_sleep_timer_id: core::sync::atomic::AtomicU64::new(0),
            timed_sleep_armed_j: core::sync::atomic::AtomicU32::new(0),
            timed_sleep_dur_ms: core::sync::atomic::AtomicU32::new(0),
            ti_on_cpu: core::sync::atomic::AtomicBool::new(false),
            running_on_cpu: core::sync::atomic::AtomicI32::new(-1),
            ti_pick_state: core::sync::atomic::AtomicU32::new(PICK_PICKED),
            terminal_switch_done: core::sync::atomic::AtomicBool::new(false),
            terminal_pacing: core::sync::atomic::AtomicBool::new(false),
            journal_handle: core::cell::Cell::new(core::ptr::null_mut()),
            task_refcnt: core::sync::atomic::AtomicU32::new(1),
            ti_a0: core::sync::atomic::AtomicU64::new(0),
            ti_a1: core::sync::atomic::AtomicU64::new(0),
            ti_a2: core::sync::atomic::AtomicU64::new(0),
            // Stack-depth fix (leg-16 KSTACK-HW: check_module_up 65.8KB,
            // hiview 76KB — both the Task-construction frame): from_fn
            // materialises these arrays through try_from_fn's wrapper
            // copies (several x 4KB for init_a0 alone). The inline-const
            // repeat writes each element in place — same values, and the
            // per-fork stack depth drops by the copy multiplier (the
            // measured gate for any KERNEL_STACK_SIZE reduction).

            // task_struct fields
            state,
            pid,
            pid_hash_next: ptr::null_mut(),
            tgid: pid, // Single-threaded process tgid == pid
            group_leader: ptr::null_mut(), // fixed up by group_leader_ptr() / new_task_at
            next_thread: ptr::null_mut(),  // fixed up by next_thread_ptr() / new_task_at
            nr_threads: AtomicU32::new(1),
            dead_threads: Spinlock::new(alloc::vec::Vec::new()),
            cred: Cred::new_init(),
            // OpenHarmony access-token bookkeeping (see
            // drivers/access_tokenid.rs); fork inherits token and clears
            // ftoken (process/fork.rs).
            access_token: AtomicU64::new(0),
            access_ftoken: AtomicU64::new(0),
            policy,
            prio,
            static_prio,
            normal_prio,
            time_slice: DEFAULT_TIME_SLICE, // Default time slice (10 clock ticks = 100ms)
            sched_entity: crate::sched::types::SchedEntity::new(),
            rt_priority: 0,
            rt_run_list: ListHead::new(),
            rt_entity: crate::sched::types::SchedRtEntity::new(),
            dl_entity: crate::sched::types::SchedDlEntity::new(),
            sched_reset_on_fork: core::sync::atomic::AtomicBool::new(false),
            kernel_stack: None,
            kernel_stack_bottom: 0,
            is_fork_child: core::sync::atomic::AtomicBool::new(false),
            fork_pt_regs: core::sync::atomic::AtomicU64::new(0),
            address_space: None,
            mm_slot_lock: Spinlock::new(()),
            active_mm: None,
            thread: crate::arch::thread::ThreadStruct::new(),
            fdtable,
            signal,
            pending,
            release_deferred: core::sync::atomic::AtomicBool::new(false),
            sigmask: 0,  // Initial signal mask is empty
            personality: 0, // PER_LINUX
            sigstack,
            sigframe_addr: 0,
            sigframe: None,
            parent: None,
            exit_code: 0,
            exit_signal: 17, // SIGCHLD default (clone exit_signal)
            stop_signal: 0,
            stop_reported: core::sync::atomic::AtomicBool::new(false),
            comm: [0u8; 16],
            pdeath_signal: 0,
            dumpable: 1, // SUID_DUMP_USER (Linux default)
            pgid: pid,
            sid: pid,
            execed: core::sync::atomic::AtomicBool::new(false),
            children: ListHead::new(),
            sibling: ListHead::new(),
            parent_children_head: ptr::null_mut(),
            wait_chldexit: super::types::WaitQueueHead::new(),
            vfork_parent: core::sync::atomic::AtomicU64::new(0),
            brk: core::sync::atomic::AtomicU64::new(0),
            oom_score_adj: core::sync::atomic::AtomicI32::new(0),
            ioprio: core::sync::atomic::AtomicI32::new(0),
            fs: Some(alloc::sync::Arc::new(crate::fs::types::FsStruct::new())),
            exe_path: Box::from(&b""[..]),
            sem_undo: Spinlock::new(alloc::vec::Vec::new()),
            itimer_ids: [
                core::sync::atomic::AtomicU64::new(0),
                core::sync::atomic::AtomicU64::new(0),
                core::sync::atomic::AtomicU64::new(0),
            ],
            itimer_virt: [AtomicU64::new(0), AtomicU64::new(0)],
            itimer_prof: [AtomicU64::new(0), AtomicU64::new(0)],
            itimer_real: [AtomicU64::new(0), AtomicU64::new(0)],
            posix_timers: Spinlock::new(alloc::vec::Vec::new()),
            rlimits: Spinlock::new(default_rlimits()),
            cpu_time_last_sigxcpu: AtomicU64::new(0),
            utime_ticks: AtomicU64::new(0),
            stime_ticks: AtomicU64::new(0),
            nvcsw: AtomicU64::new(0),
            nivcsw: AtomicU64::new(0),
            min_flt: AtomicU64::new(0),
            maj_flt: AtomicU64::new(0),
            // Per-task scheduler statistics (T7 perf-support slice,
            // /proc/[pid]/schedstat): on-cpu runtime ns, wait-on-rq ns,
            // timeslices granted — plus the two GRQ-locked stamps the
            // __schedule edges maintain (0 = window closed). All
            // atomics: the file reads them without the GRQ lock.
            schedstat_runtime_ns: AtomicU64::new(0),
            schedstat_wait_ns: AtomicU64::new(0),
            schedstat_timeslices: AtomicU64::new(0),
            schedstat_wait_start: AtomicU64::new(0),
            schedstat_exec_start: AtomicU64::new(0),
            ext: TaskExtHandle::new(),
            pick_ring: [const { AtomicU64::new(0) }; 16],
            pick_ring_seq: AtomicU64::new(0),
            pick_visit_n: AtomicU64::new(0),
            pick_refuse_why: [const { AtomicU64::new(0) }; 5],
            pick_first_refuse_jif: AtomicU64::new(0),
            pick_n: AtomicU64::new(0),
            pick_last_jif: AtomicU64::new(0),
            wake_n: AtomicU64::new(0),
            wake_last_jif: AtomicU64::new(0),
            wait_open_ns: AtomicU64::new(0),
            wait_last_ns: AtomicU64::new(0),
            wait_sum_ns: AtomicU64::new(0),
            wait_max_ns: AtomicU64::new(0),
            ns_mount: None,
            ns_uts: None,
            ns_ipc: None,
            ns_pid: None,
            ns_pid_for_children: None,
            ns_net: None,
            ns_user: None,
            ns_local_pid: pid,
            ns_upids: alloc::vec::Vec::new(),
            seccomp_mode: AtomicU32::new(0),
            seccomp_filter: Spinlock::new(None),
            no_new_privs: AtomicBool::new(false),
            cgroup_ptr: ptr::null_mut(),
            cgroup_mem_charged: AtomicU64::new(0),
        };

        // Initialize children and sibling lists (must be after struct construction)
        task.children.init();
        task.sibling.init();
        task.rt_run_list.init();
        task.pid_hash_next = ptr::null_mut();

        task
    }

    /// Construct idle task at the given task slot.
    ///
    /// The Task value is built by the safe struct-literal constructor
    /// ([`Task::new`]) and placed into the slot with ONE framework edge
    /// ([`crate::process::unsafe::place_task`], INV-PROC-PLACE-1).
    /// The compiler checks the value's field completeness, retiring the
    /// hand-maintained `offset_of` write ladder and its drift history
    /// (sc130/R9-1/R68a: a field silently missing from the hand list
    /// read the recycled page's previous life — the non-zeroing buddy
    /// made every miss a live bug).
    ///
    /// Contract: `ptr` is a fresh, aligned, Task-sized, unaliased slot
    /// (INV-PROC-PLACE-1).
    pub fn new_idle_at(ptr: *mut Task) {
        let mut task = Self::new(0, SchedPolicy::Idle);
        // Idle deltas from the generic constructor: the self-ring thread
        // group (single member), the all-CPUs mask, no pgid/sid, and the
        // idle loop entry.
        task.group_leader = ptr;
        task.next_thread = ptr;
        task.cpus_allowed = core::sync::atomic::AtomicU32::new(!0u32);
        task.pgid = 0;
        task.sid = 0;
        // Idle task entry point — runs schedule() loop with UART polling.
        // Note: intentionally uses schedule()-based idle (not raw WFI loop)
        // because WFI in QEMU TCG can starve the IO thread when stdin
        // is a blocking pipe, preventing timer interrupt delivery.
        super::hotpath::thread_set_entry(
            &mut task.thread,
            crate::sched::hotpath::cpu_idle_loop as u64,
            0, // set when the kernel stack is allocated
        );
        Self::register_slot_deferred_sizes();
        crate::process::r#unsafe::place_task(ptr, task);
        // INV-PROC-PLACE-2: the self-referential list heads are
        // re-anchored inside place_task (the single placement choke
        // point) — nothing constructor-side needed here.
    }

    /// Slot-size registrations shared by both constructors.
    ///
    /// Teardown-race guard (mm/deferred_free.rs): procfs walkers borrow
    /// the TASK unpinned (find_task_by_pid); the reaper on another CPU
    /// can free it mid-read. free_task_slot poisons the page before
    /// dealloc, so a time-deferred free leaves late readers looking at
    /// poison values (checked by the is_plausible_task_ptr guard
    /// family) instead of heap-reuse garbage pointers (the walker #GP
    /// family). The walker path's other unpinned Arc borrows — fdtable
    /// (generate_maps resolves file paths per VMA) and the shared
    /// signal struct (generate_status reads ign/cgt masks) — get the
    /// same 250ms hold: sizes are the ArcInner block (2 refcount
    /// words) + the struct.
    fn register_slot_deferred_sizes() {
        crate::mm::hotpath::register_deferred_size(core::mem::size_of::<Task>());
        crate::mm::hotpath::register_deferred_size(
            2 * core::mem::size_of::<usize>() + core::mem::size_of::<crate::fs::types::FdTable>(),
        );
        crate::mm::hotpath::register_deferred_size(
            2 * core::mem::size_of::<usize>() + core::mem::size_of::<crate::signal::SignalStruct>(),
        );
    }

    /// Construct a normal task at the given task slot.
    ///
    /// Same placement-new doctrine as [`Task::new_idle_at`]: the value
    /// comes from the safe constructor and lands in the slot through
    /// the single audited edge (INV-PROC-PLACE-1); the 293 raw-pointer
    /// field writes this body used to carry are gone.
    ///
    /// Returns false when the kernel-stack allocation fails (R10-9) —
    /// the task must then be discarded by the caller. The failure
    /// return sits AFTER the whole Task is placed (every field
    /// initialized), keeping free_task_slot's drop_in_place sound (the
    /// sched.rs contract).
    pub fn new_task_at(ptr: *mut Task, pid: Pid, policy: SchedPolicy) -> bool {
        let mut task = Self::new(pid, policy);
        // Clone-path deltas from the generic constructor:
        // - R52: TASK_NEW, not RUNNING — the task is hash-visible from
        //   pid_hash_insert until enqueue_task, and must be unwakeable in
        //   that window (see TaskState::TASK_NEW). enqueue_task_locked
        //   flips it to RUNNING atomically with the class insert.
        // - Thread group: single-member ring pointing at itself until a
        //   CLONE_THREAD child joins via thread_group_join().
        // - pgid/sid stay 0: init.rs sets pid 1's (LTP getpgid01), and
        //   fork copies the parent's before the child runs.
        // - The constructor time slice (100 ticks), as before.
        task.state = AtomicU32::new(TaskState::TASK_NEW);
        task.group_leader = ptr;
        task.next_thread = ptr;
        task.pgid = 0;
        task.sid = 0;
        task.time_slice = HZ;
        Self::register_slot_deferred_sizes();
        crate::process::r#unsafe::place_task(ptr, task);
        // INV-PROC-PLACE-2 (the beeb17524 lesson, enforced at the edge):
        // Task::new's ListHead::init() self-points children/sibling/
        // rt_run_list at the CONSTRUCTOR TEMPORARY's stack address, and
        // build-then-place kills that temporary — the old offset_of
        // ladder initialized these heads IN PLACE at the final address;
        // the Round-B rewrite silently lost it and every later list op
        // dereferenced stale stack memory (the fio null-walk / user-
        // range-node corruption family). The re-anchor now lives INSIDE
        // place_task (the single placement choke point), so no
        // constructor route can forget it. group_leader/next_thread
        // above write the FINAL slot address explicitly, which is its
        // own correct anchor.

        // Allocate kernel stack
        let t = crate::process::r#unsafe::borrow_task_mut(ptr);
        if t.alloc_kernel_stack().is_none() {
            // R10-9: do NOT continue — the task would context-switch to
            // sp=0 and kernel_stack_bottom stays uninitialized. Propagate
            // the failure so alloc_task_slot unwinds.
            use crate::console::putchar;
            const MSG_ERR: &[u8] = b"Task::new_task_at: failed to allocate kernel stack\n";
            for &b in MSG_ERR {
                putchar(b);
            }
            return false;
        }
        // Scribble hunter (rux:scribble (retired debug=scribble)): track this Task's scheduling
        // fields from birth. No-op unless the runtime switch is on.
        #[cfg(feature = "x86_64")]
        crate::debug::scribble::register(ptr);
        true
    }

    /// Get process state
    #[inline]
    pub fn state(&self) -> TaskState {
        TaskState::new(self.state.load(Ordering::Relaxed))
    }

    /// Set process state
    #[inline]
    pub fn set_state(&self, state: TaskState) {
        self.state.store(state.bits(), Ordering::Release);
    }

    /// Check if process is in specified state
    #[inline]
    pub fn is_state(&self, flag: u32) -> bool {
        self.state.load(Ordering::Relaxed) & flag != 0
    }

    /// Process sleep and wake mechanism

    /// Put current process to sleep
    ///
    /// After calling this function, process enters sleep state and triggers scheduling
    ///
    /// # WARNING — raw protocol, lost-wakeup prone (R33)
    /// `wake_up` drops every wake that arrives while the target is not yet
    /// `is_sleeping()`. A condition checked BEFORE this call has a window
    /// (check → set_state) in which the one-shot waker fires and is
    /// discarded — the sleeper then blocks forever. In-kernel callers must
    /// use the state-first + re-check discipline (set_state, RE-CHECK the
    /// condition, then schedule — see `sys_rt_sigtimedwait` /
    /// `nanosleep_impl`) or a wait queue (`WaitQueueHead::prepare_to_wait`).
    /// This function performs no re-check and must not be called with an
    /// only-once waker armed.
    ///
    /// # Arguments
    /// - `state`: Sleep state (TaskState::INTERRUPTIBLE or TaskState::UNINTERRUPTIBLE)
    ///
    /// # Safety
    /// After calling this function, current process will be scheduled out until woken
    ///
    /// # Example
    /// ```no_run
    /// # use rux::process::types::TaskState;
    /// // Interruptible sleep (can be woken by signal)
    /// Task::sleep(TaskState::new(TaskState::INTERRUPTIBLE));
    ///
    /// // Uninterruptible sleep
    /// Task::sleep(TaskState::new(TaskState::UNINTERRUPTIBLE));
    /// ```
    #[inline(never)]
    pub fn sleep(state: TaskState) {
        // Set current process to sleep state
        if let Some(current) = crate::sched::hotpath::current() {
            // SAFETY: current() returns the currently running task's pointer, which is
            // guaranteed valid for the calling task.

                current.set_state(state);

        }

        // Trigger scheduling, select other process to run
        // R54: schedule() now restores the caller's SIE state; wait-path callers re-arm explicitly (semaphore.rs discipline) so ticks/IPIs reach this CPU across the wait loop.
        crate::arch::cpu::restore_irq(true);
        crate::sched::hotpath::schedule();
    }

    /// Wake up process
    ///
    ///
    /// Wake process from sleep state, making it schedulable again
    ///
    /// # Arguments
    /// - `task`: Process to wake
    ///
    /// # Returns
    /// - true: Successfully woken
    /// - false: Process not in sleep state
    ///
    /// # Example
    /// ```no_run
    /// # use rux::sched;
    /// if let Some(child) = sched::find_task_by_pid(2) {
    ///     sched::wake_up_process(child);
    /// }
    /// ```
    /// E8-REPAR: plausibility screen for a raw Task* harvested from a
    /// parent chain or children list (GNOME final8 KERNPANIC at
    /// guest+3286s: reparent_children_to_init's subreaper walk
    /// dereferenced `(*pp).signal` on a freed ancestor and faulted at
    /// 0xb5d66 — epc=atomic_load<u8> exit.rs:954). Two screens, in the
    /// spirit of the WP2/R15-6 guards in wake_up but tightened to the
    /// kernel linear map: (1) address window — every Task is
    /// heap-allocated inside [PAGE_OFFSET, PAGE_OFFSET+RAM), so integer
    /// sentinels are rejected on both sides AND anything that passes is
    /// mapped memory; (2) the TASK_POISON canary free_task_slot writes
    /// at the compile-time state/pid offsets (catches
    /// freed-not-yet-reused pages). A page freed AND recycled into
    /// non-Task data can still pass — the durable fix is that nothing
    /// writes a stale pointer into a parent field anymore (pinning +
    /// atomic move in reparent_children_to_init); this screen degrades
    /// any residual into a bail-to-init instead of a panic.
    pub fn is_plausible_task_ptr(task: *const Task) -> bool {
        // WINDOW = kernel linear map ∪ the boot init task's static
        // storage. Every Task is heap-allocated inside [PAGE_OFFSET,
        // PAGE_OFFSET+RAM) except init's MaybeUninit<Task> in .bss
        // (init.rs — its exact address is exported, so no generous
        // image-window guess that could admit unmapped holes).
        // Restricting to these means integer sentinels are rejected on
        // BOTH sides (u32::MAX zero-extended sits below the linear map;
        // 0xFFFFFFFFFFFFFFFF — which passes wake_up's looser low bound
        // and wraps the poison probe into badaddr=0x57, caught by the
        // unit test — sits above it), and every pointer that DOES pass
        // is inside mapped memory, so the poison reads below cannot
        // fault. 64GiB linear span: generous against RAM growth.
        const MAP_BASE: usize = crate::arch::mm::memory_layout::PAGE_OFFSET;
        const MAP_SPAN: usize = 0x10_0000_0000; // 64 GiB
        let a = task as usize;
        if task.is_null()
            || ((a < MAP_BASE || a >= MAP_BASE + MAP_SPAN)
                && a != crate::init::init_task_storage_addr())
        {
            return false;
        }
        let slot = task as *const u8;
        // Reads at compile-time Task offsets; the range screen above
        // already proved the address is inside the kernel linear map, so
        // the loads cannot take a user-range page fault.
        let state = crate::process::r#unsafe::read_volatile_u32(
            crate::process::r#unsafe::slot_ptr(slot as *mut u8, task_offsets::TASK_STATE)
                as *const u32,
        );
        let pid = crate::process::r#unsafe::read_volatile_u32(
            crate::process::r#unsafe::slot_ptr(slot as *mut u8, task_offsets::TASK_PID)
                as *const u32,
        );
        state != crate::sched::hotpath::TASK_POISON && pid != crate::sched::hotpath::TASK_POISON
    }

    #[inline(never)]
    pub fn wake_up(task: *mut Task) -> bool {
        Self::wake_up_impl(task, None, false)
    }

    /// WF_SYNC wake (reference try_to_wake_up with WF_SYNC, used by
    /// binder's wake_up_interruptible_sync sites): identical state
    /// transition and enqueue; only the DISPATCH differs — see the
    /// sync leg in wake_up_impl's dispatch (66th session).
    pub fn wake_up_sync(task: *mut Task) -> bool {
        Self::wake_up_impl(task, None, true)
    }

    /// INV-LOCK-5 load-bearing twin of [`wake_up`]: wake a task while a
    /// futex bucket lock is held (futex_wake's R12-2 section). The
    /// proof — minted by the bucket guard — is required all the way
    /// down to the GRQ acquire (`wake_up_enqueue_nested` ->
    /// `lock_irqsave_nested`), so the bucket->GRQ nesting is a compile
    /// fact, not a review comment. Same body as `wake_up` (shared
    /// `wake_up_impl`); the proof argument is zero-sized.
    #[inline(never)]
    pub fn wake_up_in_futex_bucket<'a>(
        task: *mut Task,
        proof: &'a crate::sync::types::NestProof<'a, crate::sync::types::FutexBucketKey>,
    ) -> bool {
        Self::wake_up_impl(task, Some(proof), false)
    }

    /// Shared wake body (`wake_up` / `wake_up_in_futex_bucket`). The
    /// `bucket` proof, when present, routes the GRQ acquire through the
    /// compile-checked nested variant (one predictable branch; the
    /// proof is zero-sized).
    #[inline(never)]
    fn wake_up_impl(
        task: *mut Task,
        bucket: Option<&crate::sync::types::NestProof<'_, crate::sync::types::FutexBucketKey>>,
        sync: bool,
    ) -> bool {
        if task.is_null() {
            // WAKE-REFUSE-PT1 (runq-strand hunt) — RETIRED (probe-retire):
            // behind rux:schedtrace (retired debug=schedtrace) like the rest of the refusal-point
            // family; also fixes a latent null-deref in the print itself.
            if crate::trace::EVENT_SCHEDTRACE.armed() {
                static RF: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
                let rn = RF.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if rn < 12 { crate::pr_warn!("WAKE-REFUSE-PT1 tid=0"); }
            }
            return false;
        }

        // WP2 (wild-pointer guard): a wake entered here with task ==
        // 0x00000000FFFFFFFF (u32::MAX zero-extended — GNOME panic at
        // guest+452s, epc=Task::pid offset 92, ra=Task::wake_up). Every
        // legitimate Task lives in the kernel linear map or the kernel
        // image; anything below the arch's floor is user range or an
        // integer sentinel misread as a pointer. Dropping the wake loses
        // one wakeup at worst; dereferencing panicked the kernel.
        // (Per-arch floor: 0xFFFFFFC000000000 is the riscv64 layout — on
        // x86_64 every legitimate linear-map Task sits at 0xFFFF8880...,
        // below that constant, so the riscv floor dropped them all and
        // stranded fork children forever. The aarch64 linear map starts at
        // 0xFFFF_C000_0000_0000 — even further below the riscv floor; with
        // the shared not(x86_64) branch every heap Task's wake was dropped
        // as "wild" and heap tasks slept forever: PID 1's shell sat parked
        // with input waiting in the ring. Each arch now uses its own
        // PAGE_OFFSET.)
        #[cfg(any(feature = "x86_64", feature = "aarch64"))]
        const WAKE_PTR_FLOOR: usize = crate::arch::mm::memory_layout::PAGE_OFFSET;
        #[cfg(feature = "riscv64")]
        const WAKE_PTR_FLOOR: usize = 0xFFFF_FFC0_0000_0000;
        if (task as usize) < WAKE_PTR_FLOOR {
            use crate::console::putchar;
            const MSG: &[u8] = b"WAKE-WILD-PTR dropped ptr=0x";
            for &b in MSG { putchar(b); }
            let mut v = task as usize;
            for _ in 0..16 {
                let n = (v >> 60) as u8;
                putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                v <<= 4;
            }
            putchar(b'\n');
            // WAKE-REFUSE-PT2 — RETIRED (probe-retire): rux:schedtrace (retired debug=schedtrace)
            // gates the refusal-point family; the wild-pointer DROP itself
            // stays unconditional (crash-class guard above).
            if crate::trace::EVENT_SCHEDTRACE.armed() {
                static RF: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
                let rn = RF.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if rn < 12 { crate::pr_warn!("WAKE-REFUSE-PT2 tid=0x{:x}", task as usize); }
            }
            return false;
        }

        // R15-6 (S-R resurrection guard): refuse to touch a FREED Task.
        // free_task_slot poisons pid with 0xDEADBEEF; without this check a
        // stale wake (unreliable RCU grace period) would first WRITE
        // set_state/set_ti_cpu into the freed (possibly reused) page, then
        // enqueue the dead pointer — the pick then context-switches onto
        // foreign data (__switch_to rets through a reused thread.ra: the
        // linear-map illegal-instruction signature). See also the guard in
        // enqueue_task_locked.
        {
            let pid = crate::process::r#unsafe::borrow_task(task).pid();
            if pid == crate::sched::hotpath::TASK_POISON {
                use crate::console::putchar;
                const MSG: &[u8] = b"WAKE-POISONED-TASK dropped\n";
                for &b in MSG { putchar(b); }
                // WAKE-REFUSE-PT3 — RETIRED (probe-retire): rux:schedtrace (retired debug=schedtrace)
                // gates the refusal-point family; the poison DROP stays.
                if crate::trace::EVENT_SCHEDTRACE.armed() {
                    static RF: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
                    let rn = RF.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    if rn < 12 { crate::pr_warn!("WAKE-REFUSE-PT3 tid={} (poison)", pid); }
                }
                return false;
            }
        }

        // SAFETY: Caller guarantees task points to a valid Task that is not currently
        // running on another CPU (or the scheduler lock prevents concurrent access).
        // The caller guarantees task points to a valid Task that is not
        // currently running on another CPU (or the scheduler lock prevents
        // concurrent access) — INV-PROC-BORROW-1's wake-path arm.
        let t = crate::process::r#unsafe::borrow_task(task);
        {
            let old_state = t.state();
            let pid = t.pid();

            // timer-gap lane: name the wake decision for pinned-FIFO
            // targets (rt2xdbg=1, 64-line budget) — the pi_min wedge
            // needs the state-gate verdict and the enqueue outcome.
            // cpu0-pinned only (mask==1): the driver thread's 50 us
            // nanosleep polls otherwise eat the whole budget.
            if matches!(
                t.policy(),
                super::types::SchedPolicy::Fifo
                    | super::types::SchedPolicy::Rr
            ) && t.cpus_allowed() == 1
                && crate::cmdline::get_param("rt2xdbg").as_deref() == Some("1")
            {
                static WUG: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                if WUG.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 64 {
                    use crate::debug::taskdump::putc_brief as putchar;
                    fn ph(mut v: u64) {
                        for _ in 0..16 {
                            let n = (v >> 60) as u8;
                            putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                            v <<= 4;
                        }
                    }
                    for &b in b"WUG pid=0x" { putchar(b); }
                    ph(pid as u64);
                    for &b in b" st=0x" { putchar(b); }
                    ph(old_state.bits() as u64);
                    for &b in b" jif=" { putchar(b); }
                    ph(crate::timer::get_jiffies());
                    putchar(b'\n');
                }
            }

            // Wake if sleeping OR stopped (R7-3): stopped tasks were
            // dequeued when they scheduled away; SIGCONT/SIGKILL reach them
            // through here — filtering only is_sleeping() made them
            // permanently unkillable.
            if old_state.is_sleeping() || old_state.contains(TaskState::STOPPED) {
                // forkprof (cmdline forkprof=1): waker-side body cost and
                // the stamp consumed at the wakee's switch-in.
                let fp = crate::debug::forkprof::on();
                let fp_t_wake = if fp { crate::debug::forkprof::now() } else { 0 };
                // cctx probe (closegap lane): select / enqueue / dispatch
                // phase sums for the sleeping-target wake path.
                let cc = crate::debug::cctx::on();
                let cc_t_sel = if cc { crate::debug::cctx::now() } else { 0 };
                crate::debug::cctx::bump(&crate::debug::cctx::WK_N);
                // Select best CPU for this task (wake-affine / least-loaded)
                let sched_class = crate::sched::hotpath::task_sched_class(t);
                let prev_cpu = t.ti_cpu();
                let target_cpu = sched_class.select_task_rq(task, prev_cpu, 0);
                crate::debug::cctx::add(&crate::debug::cctx::WK_SEL_NS, cc_t_sel);
                let cc_t_enq = crate::debug::cctx::now();

                // Update task's CPU if it changed.
                // R13-5: never steer a task that is still executing (on_cpu
                // set from pick until __switch_to saves its context) — see
                // the deferred-notify steering fix.
                // R49 (on_cpu clear-protocol gap — the seed): the old
                // UNLOCKED `!on_cpu()` check-then-write raced the pick
                // path: this CPU passes the gate, another CPU's racing
                // wake links the task, a third picks it
                // (mark_picked_on_cpu + context_switch's ti_cpu stamp),
                // and THIS CPU's delayed steer store still lands while
                // the task executes — tp->ti_cpu then no longer names the
                // hardware CPU, cpu_id()/this_cpu() resolve the WRONG
                // PER_CPU slot on that CPU, and __schedule switches
                // context against a task running elsewhere (the R13-1
                // engine; the round-49 permanent on_cpu=1 phantom family).
                // The on_cpu re-check now runs under the GRQ lock, which
                // excludes every concurrent pick.
                if target_cpu != prev_cpu {
                    crate::sched::hotpath::steer_task_cpu(task, target_cpu);
                }

                // The sched:sched_waking / sched:sched_wakeup tracepoints
                // (the reference emits waking before the enqueue attempt
                // and wakeup only after it succeeded). One `armed()`
                // check each when off; the comm copy sits behind the
                // gate, so the disabled wake path marshals nothing.
                if crate::trace::EVENT_SCHED_WAKING.armed() {
                    let words = crate::trace::comm_to_words(&*t.comm());
                    crate::trace::record(
                        &crate::trace::EVENT_SCHED_WAKING,
                        [
                            words[0],
                            words[1],
                            pid as u64,
                            t.prio() as u64,
                            target_cpu as u64,
                            0,
                            0,
                            0,
                            0,
                            0,
                        ],
                    );
                }

                // R33 (A-family wedge — dead-task resurrection): transition to
                // RUNNING + enqueue ATOMICICALLY under the GRQ lock
                // (sched::wake_up_enqueue). The old unlocked sequence raced
                // do_exit: this wake's filter above read INTERRUPTIBLE; another
                // CPU then woke the task, it ran and exited (ZOMBIE + dequeue +
                // final schedule + parent reap freed its kernel stack), and our
                // stale set_state(RUNNING) + enqueue re-queued the dead task
                // anyway. A third CPU picking it context-switched onto the freed
                // stack (__switch_to loads thread.sp into sp) — the observed
                // trap-with-sp-on-heap-address full-system wedge. The locked
                // re-check refuses ZOMBIE/DEAD/RUNNING.
                if !crate::sched::hotpath::wake_up_enqueue_opt(task, bucket) {
                    // W2 (sc78): a SLEEPING target's enqueue+heal both
                    // refused — per the R39/R41/R42 design this is
                    // unreachable, yet the binder ring proves replies sit
                    // unconsumed on parked readers' todos for minutes.
                    // Rate-limited branch report so the refusing guard
                    // names itself live.
                    // r24 extension (kill10 wedge): capture the refusing
                    // guard's own discriminators under the lock — class
                    // linkage, owning busy/idle slots, exec clock, quantum —
                    // so "linked but starved" vs "orphaned live pick mark"
                    // vs "lying busy slot" are told apart on the line.
                    // FWK (futex-lostwake): a target that is no longer
                    // SLEEPING at the recheck is the benign redundant
                    // re-wake class — an earlier wake (or signal) already
                    // made it runnable (state RUNNING, executing or
                    // enqueued), so this wake's information is delivered;
                    // skip the report so the hunt line stays strand-only.
                    // (on_cpu alone was not enough: a just-enqueued task
                    // is RUNNING-but-not-yet-executing and flips on_cpu
                    // between the gate and the marshal.)
                    if t.state().is_sleeping() {
                        // wake-fastrepro gate tightening: a target that is
                        // MID-PICK (on_cpu set and a live busy slot backs
                        // the claim) is about to be switched to — the
                        // in-flight pick delivers this wake's information
                        // by resuming the task. The refusal here is the
                        // enqueue's double-insert guard doing its job, not
                        // a lost wake; printing it floods the census with
                        // boot-settle races (gate-r1: pid 314 at jif 11,
                        // leg otherwise clean, max_gap 1 ms over 6060
                        // parks). Pre-check the two census discriminators
                        // before the budgeted report.
                        let mid_pick = t.on_cpu() && {
                            let mut backed = false;
                            for c in 0..crate::config::MAX_CPUS {
                                if crate::sched::hotpath::cpu_state(c).current
                                    == task
                                {
                                    backed = true;
                                    break;
                                }
                            }
                            backed
                        };
                        // 121st (user-GUI storm): the first refusal raced
                        // a TRANSIENT of the double-insert guard's window —
                        // the census WW-PROBE has proven that a re-issued
                        // wake in exactly this state links 0->1 instantly.
                        // Retry ONCE, bounded, before reporting: on link,
                        // the wake is delivered (nudge its last CPU like
                        // the requeue path at wake_up_enqueue_opt's call
                        // below); only a refused retry is a real W2.
                        if !mid_pick {
                            let retry_n =
                                crate::sched::hotpath::wake_up_enqueue_opt(task, bucket);
                            if retry_n {
                                crate::sched::hotpath::resched_cpu(t.ti_cpu() as usize);
                                crate::pr_debug!(
                                    "W2-RETRY-LINKED pid={} (transient guard window)",
                                    pid
                                );
                                return true;
                            }
                        }
                        static W2_N: core::sync::atomic::AtomicU32 =
                            core::sync::atomic::AtomicU32::new(0);
                        // W2 print + monopolist capture — RETIRED
                        // (probe-retire): the wake-refusal census COUNTERS
                        // stay always-on (merge-gate band); the per-event
                        // serial lines move behind rux:schedtrace (retired debug=schedtrace).
                        if crate::trace::EVENT_SCHEDTRACE.armed()
                            && !mid_pick
                            && W2_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 16
                        {
                            let w2 = crate::sched::hotpath::w2_refusal_census(task);
                            // 118th: name the target (the pk=1 monopolist
                            // census lines were anonymous).
                            let cb = t.comm();
                            let cl = cb.iter().position(|&b| b == 0).unwrap_or(16);
                            let comm = core::str::from_utf8(&cb[..cl]).unwrap_or("?");
                            crate::pr_warn!(
                                "W2-WAKE-REFUSED-SLEEPING pid={} ({}) state={:#x} on_cpu={} pk={} cpu={} {}",
                                pid,
                                comm,
                                t.state().bits(),
                                t.on_cpu(),
                                t.pick_state(),
                                t.ti_cpu(),
                                w2
                            );
                            // debug-migrate: the refusal also records into
                            // the unified framework (rux:schedtrace
                            // kind=WAKE_REFUSED — the event is also the
                            // family's arm switch).
                            crate::trace::trace_schedtrace(
                                1,
                                pid as u32,
                                t.state().bits() as u64,
                                t.ti_cpu() as u64,
                            );
                                // 118th monopolist capture: a RUNNING+on_cpu refusal
                                // target with preempt_count>=2 is a REAL leaked-base
                                // incumbent (the sysevent_source shape — userspace
                                // last rip, honest busy slot, every blocking syscall
                                // degenerating into the park-refusal poll). One-shot
                                // per boot: IPI its CPU for the interrupted RIP +
                                // live stack — that names the kernel loop the leak
                                // rides in.
                                #[cfg(feature = "x86_64")]
                                {
                                    static MONO_DONE: core::sync::atomic::AtomicBool =
                                        core::sync::atomic::AtomicBool::new(false);
                                    let pc = t.preempt_count();
                                    if pc >= 2
                                        && t.on_cpu()
                                        && !MONO_DONE.swap(true, core::sync::atomic::Ordering::AcqRel)
                                    {
                                        let cpu = t.ti_cpu().max(0) as usize;
                                        crate::pr_warn!(
                                            "MONOPOLIST-CAPTURE pid={} ({}) preempt={} cpu={} — remote bt follows",
                                            pid,
                                            comm,
                                            pc,
                                            cpu
                                        );
                                        let _ = crate::arch::ipi::remote_backtrace(cpu, "monopolist-w2");
                                    }
                                }
                            }
                        }
                    // WAKE-REFUSE-PT4 — RETIRED (probe-retire):
                    // rux:schedtrace (retired debug=schedtrace) gates the refusal-point family.
                    if crate::trace::EVENT_SCHEDTRACE.armed() {
                        static RF4: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
                        let rn = RF4.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                        if rn < 12 {
                            crate::pr_warn!("WAKE-REFUSE-PT4 tid={} state={:#x}", t.pid(), t.state().bits());
                        }
                    }
                    return false;
                }

                // Dispatch the woken task: prefer an idle CPU within the
                // wakee's affinity (the reference's select_idle_sibling +
                // remote-enqueue shape); only when none is idle, set
                // need_resched on the wakee's previous CPU.
                //
                // Why this matters for IPC throughput (perf-ipc lane,
                // ipcdisc2 timeline evidence): the old unconditional
                // resched of the previous CPU pokes the WAKER's own (busy)
                // CPU, which preempts the sender at trap-exit and
                // serializes sender and receiver into a preempt ping-pong
                // on one CPU while the other CPU stays parked — a
                // 74-byte unix-stream record cost ~275 us of dance
                // (wake 19us + preempt-switch 56us + receiver cycle
                // 131us + sender cycle 86us) against the reference's
                // ~2.5 us with the receiver drained concurrently on the
                // idle CPU. Dispatching the wakee to the idle CPU lets
                // the sender run unmolested (its trap-exit preempt never
                // fires — no NEED_RESCHED on its CPU) and the receiver
                // runs in parallel. The GLOBAL run queue keeps the IPI
                // advisory: whichever CPU schedules first picks the task,
                // so a sender that blocks immediately after the wake
                // (pipe echo, futex handoff) still gets the in-place
                // handoff from its own schedule().
                let allowed = t.cpus_allowed();
                crate::debug::cctx::add(&crate::debug::cctx::WK_ENQ_NS, cc_t_enq);
                let cc_t_disp = crate::debug::cctx::now();
                // 105th wake-dispatch census (see sched::WAKE_DISP): the
                // dispatch outcome + idle mask at wake entry, read back by
                // the binder QPICK hunt. Relaxed stores; diagnostic only.
                crate::sched::hotpath::wake_idle_mask_set(crate::sched::hotpath::grq_idle_mask());
                // R3 (rt-r3 lane): RT/DL wakes steer the idle claim to the
                // wakee's PLACEMENT cpu (the reference's select_task_rq_rt
                // + check_preempt_curr both operate on the placement rq; a
                // timer hardirq fires on the sleeper's own cpu, so the
                // placement claim makes the dispatch a self-poke consumed
                // at the IRQ exit instead of a rotated idle cpu that needs
                // an IPI + halt-exit). Fair classes keep the rotating
                // claim unchanged — the D-14 divergence stands for CFS.
                let rt_prefer = match t.policy() {
                    super::types::SchedPolicy::Fifo
                    | super::types::SchedPolicy::Rr
                    | super::types::SchedPolicy::Deadline => t.ti_cpu(),
                    _ => -1,
                };
                match crate::sched::hotpath::claim_idle_cpu_for_prefer(allowed, rt_prefer) {
                    Some(idle_cpu) => {
                        crate::sched::hotpath::wake_disp_set(1);
                        crate::sched::hotpath::resched_cpu(idle_cpu);
                    }
                    None => {
                        // No idle CPU: poke a busy CPU only when the wakee
                        // wins the wakeup-preemption comparison (see
                        // preempt_check_wake) — the old unconditional
                        // resched of the previous CPU forced a preempt
                        // dance per record on same-CPU sender/receiver
                        // pairs.
                        //
                        // 66th (WF_SYNC parity): a SYNC wake stacks the
                        // wakee with the WAKER instead — the reference's
                        // wake_affine(sync) places it on this CPU (2-cpu
                        // both-busy: nr_running==1 → this_cpu; weight-tie
                        // bias → this_cpu), so the pair runs in strict
                        // handoff order: the waker finishes its BWR and
                        // parks, THEN the wakee runs. Poking the wakee's
                        // previous CPU here would run the wakee in
                        // parallel with the still-running waker — a
                        // third thread barges exactly the reentrancy
                        // windows (FFRT mutex unlock→waiter-recheck) the
                        // reference keeps closed for sync handoffs.
                        if sync {
                            crate::sched::hotpath::wake_disp_set(2);
                            crate::sched::hotpath::preempt_check_wake_sync(task);
                        } else {
                            crate::sched::hotpath::wake_disp_set(3);
                            crate::sched::hotpath::preempt_check_wake(task);
                        }
                    }
                }
                crate::debug::cctx::add(&crate::debug::cctx::WK_DISP_NS, cc_t_disp);

                // sched:sched_wakeup — the enqueue succeeded, the task is
                // runnable on target_cpu (the reference's post-enqueue
                // record; see the sched_waking note above for the gating).
                if crate::trace::EVENT_SCHED_WAKEUP.armed() {
                    let words = crate::trace::comm_to_words(&*t.comm());
                    crate::trace::record(
                        &crate::trace::EVENT_SCHED_WAKEUP,
                        [
                            words[0],
                            words[1],
                            pid as u64,
                            t.prio() as u64,
                            target_cpu as u64,
                            0,
                            0,
                            0,
                            0,
                            0,
                        ],
                    );
                }

                if fp {
                    crate::debug::forkprof::add(crate::debug::forkprof::P_WAKE_BODY, fp_t_wake);
                    crate::debug::forkprof::stamp_wake(pid as u32);
                }
                true
            } else {
                // R46 (B-form phantom — refused-wake mirror of R41/R42):
                // RUNNING is legitimate only while the task is on a CPU
                // (on_cpu == 1 from pick until __switch_to saves the
                // outgoing context) or linked on a class queue. This
                // branch refused EVERY wake of a task whose state reads
                // RUNNING, so a RUNNING task that is on NO CPU (a write
                // into the state word from outside the scheduler — note
                // RUNNING == 0 — or a future regression; the scheduler's
                // own transitions cannot produce it) stayed unschedulable
                // forever: no queue holds it, no wake reaches it. The
                // on_cpu read here is an unlocked HINT only; route the
                // suspect into wake_up_enqueue, whose GRQ-locked re-check
                // verifies actual linkage and heals only the genuine
                // divergence (a queued or on-CPU RUNNING task is still
                // refused there, unchanged).
                // R49: the hint no longer requires on_cpu == 0 — an
                // ORPHANED pick mark (RUNNING, unlinked, owned by no
                // CPU's current slot — the 344/345/356 morphology) reads
                // on_cpu == 1 but is exactly as unwakeable, and only the
                // locked re-check can tell an orphaned mark from one that
                // a CPU genuinely owns mid-pick. The hint routes both;
                // wake_up_enqueue's authoritative current-slot scan sorts
                // them apart.
                if old_state == TaskState::new(TaskState::RUNNING) {
                    // r11 (reference wake contract): try_to_wake_up
                    // returns 1 only when it transitioned a non-RUNNING
                    // task to running — the reference's entry filter
                    // (`p->__state & state`) refuses a RUNNING target
                    // outright and returns 0. Keep the R46/R49 phantom
                    // RELINK (RUNNING ∧ unlinked ∧ unowned is a
                    // divergence shape this tree's phantom families can
                    // produce; relinking restores the reference invariant
                    // that a RUNNING task is queued or on a CPU) and its
                    // resched nudge, but report the reference's answer:
                    // nobody "woke" a task that was already RUNNING. The
                    // old true-return was an API divergence — the
                    // sleep_wakeup suite hit it deterministically once
                    // the first wake's enqueue had been picked+dequeued
                    // elsewhere, leaving the heal as the only arm.
                    if crate::sched::hotpath::wake_up_enqueue_opt(task, bucket) {
                        // Linked again — nudge its last CPU so it is
                        // picked promptly instead of waiting for the
                        // next idle tick.
                        crate::sched::hotpath::resched_cpu(t.ti_cpu() as usize);
                    }
                    // WAKE-REFUSE-PT5 — RETIRED (probe-retire): this is
                    // the loudest refusal-point arm (a benign scheduler
                    // race outcome, ~12 lines every ordinary boot); behind
                    // rux:schedtrace (retired debug=schedtrace). The heal + resched logic is unchanged.
                    if crate::trace::EVENT_SCHEDTRACE.armed() {
                        static RF5: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
                        let rn = RF5.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                        if rn < 12 {
                            crate::pr_warn!("WAKE-REFUSE-PT5 tid={} state={:#x}", t.pid(), t.state().bits());
                        }
                    }
                    return false;
                }
                false
            }
        }
    }

    /// Get PID
    #[inline]
    pub fn pid(&self) -> Pid {
        self.pid
    }

    /// Get a snapshot of one resource limit `(rlim_cur, rlim_max)`.
    ///
    /// Convenience wrapper over the rlimits table for the enforcement
    /// points (alloc_fd / mmap / brk / fork / scheduler_tick). Uses
    /// lock_irqsave: scheduler_tick reads it from IRQ context, so every
    /// task-context acquisition of the same lock must be irqsave as well
    /// (an interrupted plain-lock holder would wedge the tick).
    pub fn rlimit(&self, resource: usize) -> (u64, u64) {
        if resource >= RLIM_NLIMITS {
            return (u64::MAX, u64::MAX); // RLIM_INFINITY for unknown ids
        }
        self.rlimits.lock_irqsave()[resource]
    }

    /// Last CPU-time second at which SIGXCPU was sent (RLIMIT_CPU).
    pub fn cpu_time_last_sigxcpu(&self) -> u64 {
        self.cpu_time_last_sigxcpu.load(Ordering::Acquire)
    }

    /// Record the CPU-time second of the latest SIGXCPU delivery.
    pub fn set_cpu_time_last_sigxcpu(&self, secs: u64) {
        self.cpu_time_last_sigxcpu.store(secs, Ordering::Release);
    }

    /// Preemptive scheduling support

    /// Decrement time slice
    ///
    ///
    /// # Returns
    /// - true: Time slice remaining
    /// - false: Time slice exhausted
    #[inline]
    pub fn tick_time_slice(&mut self) -> bool {
        if self.time_slice > 0 {
            self.time_slice -= 1;
            true
        } else {
            false
        }
    }

    /// Reset time slice
    ///
    /// Called when process is rescheduled to CPU
    #[inline]
    pub fn reset_time_slice(&mut self) {
        self.time_slice = DEFAULT_TIME_SLICE;
    }

    /// Check if time slice exhausted
    #[inline]
    pub fn time_slice_expired(&self) -> bool {
        self.time_slice == 0
    }

    /// Get remaining time slice
    #[inline]
    pub fn get_time_slice(&self) -> u32 {
        self.time_slice
    }

    /// Set time slice
    #[inline]
    pub fn set_time_slice(&mut self, slice: u32) {
        self.time_slice = slice;
    }

    /// End of preemptive scheduling support

    // ==================== CFS scheduling support ====================

    /// Get CFS scheduling entity
    #[inline]
    pub fn sched_entity(&self) -> &crate::sched::types::SchedEntity {
        &self.sched_entity
    }

    /// Get CFS scheduling entity (mutable reference)
    #[inline]
    pub fn sched_entity_mut(&mut self) -> &mut crate::sched::types::SchedEntity {
        &mut self.sched_entity
    }

    /// Get scheduling policy
    #[inline]
    pub fn policy(&self) -> SchedPolicy {
        self.policy
    }

    /// Set scheduling policy
    #[inline]
    pub fn set_policy(&mut self, policy: SchedPolicy) {
        self.policy = policy;
    }

    /// Get static priority
    #[inline]
    pub fn static_prio(&self) -> i32 {
        self.static_prio
    }

    /// Get dynamic priority
    #[inline]
    pub fn prio(&self) -> i32 {
        self.prio
    }

    /// Set dynamic priority
    #[inline]
    pub fn set_prio(&mut self, prio: i32) {
        self.prio = prio;
    }

    /// Get RT priority (0-99, lower value = higher priority)
    #[inline]
    pub fn rt_priority(&self) -> u32 {
        self.rt_priority
    }

    /// Set RT priority
    #[inline]
    pub fn set_rt_priority(&mut self, prio: u32) {
        self.rt_priority = prio.min(99);
    }

    /// Get RT scheduling entity
    #[inline]
    pub fn rt_entity(&self) -> &crate::sched::types::SchedRtEntity {
        &self.rt_entity
    }

    /// Get RT scheduling entity (mutable reference)
    #[inline]
    pub fn rt_entity_mut(&mut self) -> &mut crate::sched::types::SchedRtEntity {
        &mut self.rt_entity
    }

    /// Get Deadline scheduling entity
    #[inline]
    pub fn dl_entity(&self) -> &crate::sched::types::SchedDlEntity {
        &self.dl_entity
    }

    /// Get Deadline scheduling entity (mutable reference)
    #[inline]
    pub fn dl_entity_mut(&mut self) -> &mut crate::sched::types::SchedDlEntity {
        &mut self.dl_entity
    }

    /// SCHED_RESET_ON_FORK request state.
    #[inline]
    pub fn sched_reset_on_fork(&self) -> bool {
        self.sched_reset_on_fork.load(core::sync::atomic::Ordering::Acquire)
    }

    /// Set/clear the SCHED_RESET_ON_FORK request (atomic — called from
    /// sched_setscheduler/sched_setattr without exclusive task access).
    #[inline]
    pub fn set_sched_reset_on_fork(&self, on: bool) {
        self.sched_reset_on_fork.store(on, core::sync::atomic::Ordering::Release);
    }

    /// Get nice value
    ///
    /// Nice value range: -20 to +19
    /// Calculated from static_prio: nice = static_prio - 120
    #[inline]
    pub fn nice(&self) -> i32 {
        self.static_prio - 120
    }

    /// Set nice value
    ///
    /// Also updates static_prio and scheduling entity weight
    pub fn set_nice(&mut self, nice: i32) {
        // Nice value range: -20 to +19
        let nice = nice.clamp(-20, 19);

        // Update static_prio
        self.static_prio = nice + 120;
        self.normal_prio = self.static_prio;
        self.prio = self.normal_prio;

        // Update scheduling entity weight
        self.sched_entity.set_nice(nice);
    }

    // ==================== Process tree management ====================

    /// Get parent process PID (PPID)
    #[inline]
    pub fn ppid(&self) -> Pid {
        match self.parent {
            Some(parent_ptr) => {
                // E8-REPAR: a thread's parent field is an unlinked
                // snapshot (thread_group_join copies leader.parent) that
                // can name a reaped-and-freed ancestor; a non-leader
                // caller should use the leader's ppid, and everyone else
                // gets 0 rather than a dereference of freed memory.
                if !Self::is_plausible_task_ptr(parent_ptr) {
                    return 0;
                }
                // parent_ptr passed the plausibility screen; for
                // leaders it was set by add_child() and remains valid
                // while the child is alive (INV-PROC-BORROW-1).
                crate::process::r#unsafe::borrow_task(parent_ptr as *mut Task).pid
            }
            None => 0, // No parent process, return 0
        }
    }

    /// Check if fork child process
    #[inline]
    pub fn is_fork_child(&self) -> bool {
        self.is_fork_child.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Set as fork child process
    #[inline]
    pub fn set_fork_child(&self, pt_regs_ptr: *const crate::arch::pt_regs::PtRegs) {
        self.is_fork_child.store(true, core::sync::atomic::Ordering::Relaxed);
        self.fork_pt_regs.store(pt_regs_ptr as u64, core::sync::atomic::Ordering::Relaxed);
    }

    /// Get fork child's PtRegs pointer
    #[inline]
    pub fn fork_pt_regs(&self) -> *const crate::arch::pt_regs::PtRegs {
        self.fork_pt_regs.load(core::sync::atomic::Ordering::Relaxed) as *const crate::arch::pt_regs::PtRegs
    }

    /// Clear fork child flag (called after child is first scheduled).
    /// Does NOT clear fork_pt_regs — the heap-allocated PtRegs is freed
    /// later in release_task().
    #[inline]
    pub fn clear_fork_child(&self) {
        self.is_fork_child.store(false, core::sync::atomic::Ordering::Relaxed);
    }

    /// Get TGID
    #[inline]
    pub fn tgid(&self) -> Pid {
        self.tgid
    }

    /// Set TGID (for CLONE_THREAD)
    #[inline]
    pub fn set_tgid(&mut self, tgid: Pid) {
        self.tgid = tgid;
    }

    // ==================== Thread group (task_struct::thread_group) ====================

    /// Thread group leader pointer; heals a NULL (pre-fixup) value to self.
    #[inline]
    pub fn group_leader_ptr(&self) -> *mut Task {
        if self.group_leader.is_null() {
            self as *const Task as *mut Task
        } else {
            self.group_leader
        }
    }

    // (2026-11-23, §7 retirement round: the parent_children_head_ptr
    // diagnostic accessor retired with debug/treehunt.rs — it was the
    // probe's only consumer. The parent_children_head field itself is
    // live list machinery and stays.)

    /// Raw group_leader field (no healing) — for re-init during teardown.
    #[inline]
    pub fn group_leader_raw(&self) -> *mut Task {
        self.group_leader
    }

    /// Set group leader (fork.rs, when joining an existing group).
    #[inline]
    pub fn set_group_leader(&mut self, leader: *mut Task) {
        self.group_leader = leader;
    }

    /// Next member in the thread group ring; heals NULL to self.
    #[inline]
    pub fn next_thread_ptr(&self) -> *mut Task {
        if self.next_thread.is_null() {
            self as *const Task as *mut Task
        } else {
            self.next_thread
        }
    }

    /// Live thread count. Authoritative on the leader.
    #[inline]
    pub fn nr_threads(&self) -> u32 {
        self.nr_threads.load(Ordering::Acquire)
    }

    /// Is this task its own group leader (or single-threaded)?
    #[inline]
    pub fn is_thread_group_leader(&self) -> bool {
        let l = self.group_leader;
        l.is_null() || l as *const Task == self as *const Task
    }

    /// Join `member` into this task's thread group (CLONE_THREAD).
    ///
    /// Head-insert into the leader's ring: leader -> member -> old_first.
    /// nr_threads is bumped on the leader. PROCESS_TREE_LOCK serializes
    /// concurrent join/leave/walk (the process-tree lock doubles as the
    /// thread-group lock; both guard task_struct linkage fields).
    ///
    /// Contract: `self` is a valid leader (its ring is well-formed),
    /// `member` is a fully-constructed task not yet in any ring, and
    /// neither is being torn down concurrently.
    pub fn thread_group_join(&mut self, member: *mut Task) {
        let _lock = PROCESS_TREE_LOCK.lock();
        let leader = self as *mut Task;
        // Caller contract above — member is a fully-constructed task;
        // the tree/ring lock is held for the whole link.
        let m = crate::process::r#unsafe::borrow_task_mut(member);
        m.set_group_leader(leader);
        m.set_tgid(self.tgid);
        // member.parent mirrors the leader's parent (threads share the real
        // parent) — but the member is NOT linked into the parent's children
        // list (wait4 only ever sees process products).
        m.parent = self.parent;
        m.parent_children_head = ptr::null_mut();
        m.next_thread = self.next_thread;
        self.next_thread = member;
        self.nr_threads.fetch_add(1, Ordering::Release);
    }

    /// Remove `member` from its thread group ring and return the leader.
    ///
    /// Returns NULL when the member is its own leader (nothing to leave).
    /// The caller must NOT be `member` itself already scheduled out.
    ///
    /// Contract: `member` is a live ring member of a well-formed ring.
    pub fn thread_group_leave(member: *mut Task) -> *mut Task {
        // Caller contract above — member is a live ring member.
        let m = crate::process::r#unsafe::borrow_task_mut(member);
        let leader = m.group_leader;
        if leader.is_null() || leader as *const Task == member as *const Task {
            return ptr::null_mut();
        }
        let _lock = PROCESS_TREE_LOCK.lock();
        // Find member's predecessor, walking from the leader (ring, no
        // tail); ring nodes stay linked under PROCESS_TREE_LOCK.
        let mut prev = leader;
        let mut guard = 0u32;
        {
            while guard < 65536 {
                let next = crate::process::r#unsafe::borrow_task(prev).next_thread;
                if next == member || next == leader {
                    break;
                }
                prev = next;
                guard += 1;
            }
            if crate::process::r#unsafe::borrow_task(prev).next_thread == member {
                crate::process::r#unsafe::borrow_task_mut(prev).next_thread = m.next_thread;
            }
        }
        // Re-init to a self-ring so later accidental walks stay bounded.
        m.next_thread = member;
        m.group_leader = member;
        // Leader is a live ring member (validated above).
        crate::process::r#unsafe::borrow_task(leader)
            .nr_threads
            .fetch_sub(1, Ordering::Release);
        // timer-gap lane: the leader may be PARKED in
        // wait_for_thread_group_death (exit_group/de_thread) waiting for
        // exactly this decrement — wake it so it re-checks nr_threads.
        // Outside PROCESS_TREE_LOCK (wake_up takes the GRQ); a no-op when
        // the leader is running or not waiting.
        let leader_ptr = leader as *mut Task;
        drop(_lock);
        if leader_ptr != member {
            let _ = Task::wake_up(leader_ptr);
        }
        leader_ptr
    }

    /// Iterate every live member of this leader's group (leader included).
    ///
    /// Collect under the ring lock what you need and act outside it: signal
    /// delivery takes pid-hash pins and the GRQ, which must not nest under
    /// PROCESS_TREE_LOCK in the reverse order anywhere.
    ///
    /// Contract: `self` is a valid task whose ring is well-formed; each
    /// `&Task` handed to `f` is valid only for the closure body (the
    /// ring lock keeps members linked for the walk — do not store).
    pub fn for_each_group_member<F>(&self, mut f: F)
    where
        F: FnMut(&Task),
    {
        let _lock = PROCESS_TREE_LOCK.lock();
        let leader = self as *const Task as *mut Task;
        // Leader is self (valid by contract); ring members stay linked
        // while PROCESS_TREE_LOCK is held (INV-PROC-BORROW-1).
        {
            f(crate::process::r#unsafe::borrow_task(leader));
            let mut t = crate::process::r#unsafe::borrow_task(leader).next_thread;
            let mut guard = 0u32;
            while !t.is_null() && t != leader && guard < 65536 {
                f(crate::process::r#unsafe::borrow_task(t));
                t = crate::process::r#unsafe::borrow_task(t).next_thread;
                guard += 1;
            }
        }
    }

    /// Get process credentials
    #[inline]
    pub fn cred(&self) -> &Cred {
        &self.cred
    }

    /// Get mutable process credentials
    #[inline]
    pub fn cred_mut(&mut self) -> &mut Cred {
        &mut self.cred
    }

    /// OH access token id (/dev/access_token_id).
    #[inline]
    pub fn access_token(&self) -> u64 {
        self.access_token.load(Ordering::Acquire)
    }

    #[inline]
    pub fn set_access_token(&self, token: u64) {
        self.access_token.store(token, Ordering::Release);
    }

    /// OH foreground token id (/dev/access_token_id).
    #[inline]
    pub fn access_ftoken(&self) -> u64 {
        self.access_ftoken.load(Ordering::Acquire)
    }

    #[inline]
    pub fn set_access_ftoken(&self, token: u64) {
        self.access_ftoken.store(token, Ordering::Release);
    }

    /// Get mutable reference to address space
    /// Note: AddressSpace has interior mutability, so &self is sufficient
    pub fn address_space_mut(&mut self) -> Option<&AddressSpace> {
        self.address_space.as_ref().map(|arc| arc.as_ref())
    }

    /// Get reference to address space.
    ///
    /// CURRENT-task use only: the running task holds its own Arc, and only
    /// its own exit/exec (same context) can drop it. For a FOREIGN task
    /// the borrow is unaccounted — the target's exit_mm can free the
    /// MmStruct (and its VMA btree) mid-walk (the /proc maps #GP family).
    /// Foreign readers must use [`Task::address_space_arc`] (refcounted
    /// pin) or [`Task::mm_ptr_peek`] (identity compare only).
    pub fn address_space(&self) -> Option<&AddressSpace> {
        self.address_space.as_ref().map(|arc| arc.as_ref())
    }

    /// Set address space.
    ///
    /// The slot swap runs under `mm_slot_lock` so a concurrent
    /// `address_space_arc()` on another CPU either clones the OLD Arc
    /// before our swap (it then holds a real reference — our drop of the
    /// old Arc cannot free the mm) or observes the new value. The old
    /// Arc is dropped AFTER releasing the lock: its Drop may run the
    /// full page-table teardown (synchronize_rcu + free_user_page_tables)
    /// and must never run under this (irqsave) spinlock.
    pub fn set_address_space(&mut self, addr_space: Option<alloc::sync::Arc<AddressSpace>>) {
        let old = {
            let _g = self.mm_slot_lock.lock_irqsave();
            // Update active_mm pointer (raw borrow mirror of the slot)
            // inside the same critical section.
            self.active_mm = addr_space.as_ref().map(|a| a.as_ref() as *const AddressSpace);
            // Teardown-context attribution (FUT-STALE family): the task
            // installing this mm is its owner; MmStruct::drop compares it
            // against the dropping context to witness deferred teardowns.
            if let Some(ref a) = addr_space {
                a.set_owner_pid(self.pid() as i32);
            }
            core::mem::replace(&mut self.address_space, addr_space)
        };
        // Refcount drop (possible last-reference teardown) outside the lock.
        drop(old);
    }

    /// Get Arc reference to address space (for CLONE_VM / foreign-task mm
    /// pinning).
    ///
    /// The clone runs under `mm_slot_lock`, making it race-free against a
    /// concurrent `set_address_space` on the owner CPU: an Arc clone can
    /// no longer land on a slot whose last reference was just dropped
    /// (the resurrected-Arc double-teardown engine). While the returned
    /// Arc is held, the MmStruct, its VMA tree and its page tables cannot
    /// be freed.
    ///
    /// NOTE: the returned Arc's DROP must not happen while holding a
    /// spinlock — if it is the last reference, MmStruct::drop runs the
    /// page-table teardown, which may schedule (grace-period wait).
    pub fn address_space_arc(&self) -> Option<alloc::sync::Arc<AddressSpace>> {
        let _g = self.mm_slot_lock.lock_irqsave();
        self.address_space.clone()
    }

    /// Peek the mm Arc's address WITHOUT taking a reference (identity
    /// comparison only — never dereference the result).
    ///
    /// For pid-hash bucket-locked scans (OOM sibling search) where an Arc
    /// clone would be dropped under the bucket spinlock: a last-reference
    /// drop there would run the mm teardown under a lock. Comparing raw
    /// Arc addresses is ABA-safe when the other side of the comparison is
    /// a live, refcount-pinned Arc.
    pub fn mm_ptr_peek(&self) -> Option<*const AddressSpace> {
        let _g = self.mm_slot_lock.lock_irqsave();
        self.address_space.as_ref().map(|a| a.as_ref() as *const AddressSpace)
    }

    /// Get active address space (for kernel threads, this is borrowed address space)
    pub fn active_mm(&self) -> Option<&AddressSpace> {
        if let Some(ref aspace) = self.address_space {
            Some(aspace.as_ref())
        } else if let Some(mm_ptr) = self.active_mm {
            // active_mm points to a borrowed AddressSpace that outlives
            // this task (the lazy-mm/active-mm discipline: the owner's
            // mm_slot keeps it referenced — INV-PROC-ACTIVEMM-1).
            Some(crate::process::r#unsafe::borrow_active_mm(mm_ptr))
        } else {
            None
        }
    }

    /// Clear active_mm (used by exit_mm during process exit)
    pub fn clear_active_mm(&mut self) {
        self.active_mm = None;
    }

    /// Set vfork parent task pointer (called in do_clone when CLONE_VFORK)
    pub fn set_vfork_parent(&self, parent: *mut Task) {
        self.vfork_parent.store(parent as u64, core::sync::atomic::Ordering::Release);
    }

    /// Clear vfork parent (called after parent is woken)
    pub fn clear_vfork_parent(&self) {
        self.vfork_parent.store(0, core::sync::atomic::Ordering::Release);
    }

    /// Get vfork parent task pointer, if any
    pub fn vfork_parent_ptr(&self) -> Option<*mut Task> {
        let val = self.vfork_parent.load(core::sync::atomic::Ordering::Acquire);
        if val != 0 { Some(val as *mut Task) } else { None }
    }
}

/// Wake the vfork parent (if any) of the given task.
///
/// Called from execve (after replacing address space) and do_exit (before ZOMBIE).
/// This completes the vfork contract: parent resumes execution.
///
/// # Safety
/// Caller must ensure `task` is a valid Task pointer and the task's
/// vfork_parent (if set) points to a valid, still-alive Task.
/// Contract: `task` is a valid task pointer (its vfork linkage fields
/// are read and cleared).
pub fn vfork_wake_parent(task: *mut Task) {
    // SAFETY: caller contract above.
    let t = crate::process::r#unsafe::borrow_task_mut(task);
    if let Some(parent) = t.vfork_parent_ptr() {
        t.clear_vfork_parent();
        crate::sched::hotpath::wake_up_process(parent);
    }
}

/// Rate gate for the preempt_count underflow tripwire: one report per
/// second of jiffies. Raw console only (putchar_no_lock) — the tripwire
/// can fire from any context, including inside printk's own lock guards,
/// so the report path must itself be lock-free and non-recursive.
fn preempt_underflow_report(
    pid: u32,
    prev: i32,
    sub: i32,
    loc: &'static core::panic::Location<'static>,
    site: usize,
) {
    static LAST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let now = crate::timer::get_jiffies();
    let last = LAST.load(core::sync::atomic::Ordering::Acquire);
    if now.wrapping_sub(last) < crate::config::KERNEL_HZ as u64 {
        return;
    }
    if LAST
        .compare_exchange(last, now, core::sync::atomic::Ordering::AcqRel, core::sync::atomic::Ordering::Acquire)
        .is_err()
    {
        return;
    }
    use crate::console::putchar_no_lock as putchar;
    fn ph(v: u32) {
        use crate::console::putchar_no_lock as putchar;
        for sh in (0..8).rev() {
            let n = ((v >> (sh * 4)) & 0xF) as u8;
            putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
        }
    }
    const MSG: &[u8] = b"\nPREEMPT-UNDERFLOW pid=";
    for &b in MSG {
        putchar(b);
    }
    ph(pid);
    const MSG2: &[u8] = b" count_was=";
    for &b in MSG2 {
        putchar(b);
    }
    ph(prev as u32);
    const MSG3: &[u8] = b" sub=";
    for &b in MSG3 {
        putchar(b);
    }
    ph(sub as u32);
    const MSG4: &[u8] = b" (unpaired decrement clamped to 0) at ";
    for &b in MSG4 {
        putchar(b);
    }
    // Drop site: file:line of the guard whose decrement underflowed.
    for &b in loc.file().as_bytes() {
        putchar(b);
    }
    const COLON: &[u8] = b":";
    for &b in COLON {
        putchar(b);
    }
    ph(loc.line());
    if site != 0 {
        const M5: &[u8] = b" lock=0x";
        for &b in M5 {
            putchar(b);
        }
        // 16-hex lock instance address (ph is 8-hex only; print two
        // halves).
        ph((site >> 32) as u32);
        ph(site as u32);
    }
    putchar(b'\n');
    // preempt-hunt (2026-11-01): the outermost-acquire record for this
    // task names the acquire whose 1->0 release this clamp adjudicates —
    // for a post-heal underflow that is the REAL guard the heal zeroed
    // (a guard held across a scheduler point); for a booking split it is
    // the phantom's home site. Rate-gate already passed above.
    if crate::trace::EVENT_LEAKHUNT.armed() {
        if let Some(task) = crate::sched::hotpath::current() {
            crate::debug::leakorigin::report_outermost(
                b"UNDERFLOW-ORIGIN",
                task as *mut Task,
            );
        }
    }
}

impl Task {
    /// Get architecture-specific thread state
    pub fn thread(&self) -> &crate::arch::thread::ThreadStruct {
        &self.thread
    }

    /// Get mutable reference to architecture-specific thread state
    pub fn thread_mut(&mut self) -> &mut crate::arch::thread::ThreadStruct {
        &mut self.thread
    }

    // ==================== thread_info style accessors ====================

    /// Get thread_info flags
    #[inline]
    pub fn ti_flags(&self) -> u32 {
        self.ti_flags.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Set thread_info flags
    #[inline]
    pub fn set_ti_flags(&self, flags: u32) {
        self.ti_flags.store(flags, core::sync::atomic::Ordering::Release);
    }

    /// Test thread_info flag bit
    #[inline]
    pub fn test_ti_flag(&self, flag: u32) -> bool {
        (self.ti_flags.load(core::sync::atomic::Ordering::Relaxed) & flag) != 0
    }

    /// Set thread_info flag bit
    #[inline]
    pub fn set_ti_flag(&self, flag: u32) {
        self.ti_flags.fetch_or(flag, core::sync::atomic::Ordering::Release);
    }

    /// Clear thread_info flag bit
    #[inline]
    pub fn clear_ti_flag(&self, flag: u32) {
        self.ti_flags.fetch_and(!flag, core::sync::atomic::Ordering::Release);
    }

    /// Check if needs rescheduling
    #[inline]
    pub fn need_resched(&self) -> bool {
        self.test_ti_flag(TIF_NEED_RESCHED)
    }

    /// Set need rescheduling flag
    #[inline]
    pub fn set_need_resched_flag(&self) {
        self.set_ti_flag(TIF_NEED_RESCHED);
    }

    /// Clear need rescheduling flag
    #[inline]
    pub fn clear_need_resched_flag(&self) {
        self.clear_ti_flag(TIF_NEED_RESCHED);
    }

    /// Check if has pending signal
    #[inline]
    pub fn has_pending_signal(&self) -> bool {
        self.test_ti_flag(TIF_SIGPENDING)
    }

    /// Set pending signal flag
    #[inline]
    pub fn set_pending_signal_flag(&self) {
        self.set_ti_flag(TIF_SIGPENDING);
    }

    /// Get preempt count (raw value)
    #[inline]
    pub fn preempt_count(&self) -> i32 {
        self.ti_preempt_count.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Increment preempt count (preemption disable)
    #[inline]
    pub fn inc_preempt_count(&self) {
        self.add_preempt_count(crate::interrupt::preempt::PREEMPT_OFFSET);
    }

    /// Decrement preempt count (preemption enable)
    #[inline]
    pub fn dec_preempt_count(&self) {
        self.sub_preempt_count(crate::interrupt::preempt::PREEMPT_OFFSET);
    }

    /// Check if preemptible
    #[inline]
    pub fn preemptible(&self) -> bool {
        self.preempt_count() == 0
    }

    /// Add value to preempt_count (atomic fetch-add)
    #[inline]
    pub fn add_preempt_count(&self, val: i32) {
        self.ti_preempt_count.fetch_add(val, core::sync::atomic::Ordering::Release);
    }

    /// Subtract value from preempt_count (atomic fetch-sub).
    ///
    /// Underflow tripwire (kernel-preemption re-landing, 2026-10-20): a
    /// preempt_count below zero is never legal — every field is a nesting
    /// counter — but a value of, say, -1 sets EVERY mask bit
    /// (`count & (HARDIRQ|SOFTIRQ|NMI)`), so `in_interrupt()` reads true
    /// for a task running in USER mode: the next user page fault is
    /// routed to no_context → KERNPANIC and parks the CPU (the sc60
    /// shape under the reverted 2cf4bdc9, where an unpaired guard drop
    /// poisoned pid 327 to -1). Instead of letting the poison persist,
    /// clamp to zero at the violating sub and report the event raw
    /// (this can fire from any context, including inside printk's own
    /// guards — no locks, direct console, rate-gated to 1/s so a
    /// storm cannot flood the UART).
    #[inline]
    pub fn sub_preempt_count(&self, val: i32) {
        self.sub_preempt_count_at(val, core::panic::Location::caller());
    }

    /// Sub with the drop site's source location for the underflow report
    /// (leak-hunt round 3b: name WHICH guard family double-drops).
    #[inline]
    pub fn sub_preempt_count_at(
        &self,
        val: i32,
        loc: &'static core::panic::Location<'static>,
    ) {
        let prev = self.ti_preempt_count.fetch_sub(val, core::sync::atomic::Ordering::Release);
        // Outermost base release (base field 1->0): clear the
        // leakorigin outermost-acquire record (preempt-hunt tap). One
        // relaxed load + compare when the probe is off.
        if val == crate::interrupt::preempt::PREEMPT_OFFSET
            && (prev & crate::interrupt::preempt::PREEMPT_MASK) == 1
        {
            crate::debug::leakorigin::note_release(self as *const Task as *mut Task);
            // phantom-bits / stress-port r3: a LIVE outermost pair just
            // completed — re-arm the run counter the tick-side heal
            // actually gates on. (The pb3 re-arm stored the superseded
            // ti_pc_zero_jif field, which nothing reads, so the
            // per-field bar never re-armed here and hot lock users
            // accumulated stale bars between absent tick samples.)
            self.ti_pc_pirq_present_run.store(
                0,
                core::sync::atomic::Ordering::Relaxed,
            );
        }
        // phantom-bits: softirq field drained to zero — the +0x100
        // acquire paired; drop the shadow so a later orphan report names
        // only still-unpaired bookings, and re-arm the sirq run counter
        // (the drain is the field's own liveness proof).
        if val == crate::interrupt::preempt::SOFTIRQ_OFFSET
            && (prev & crate::interrupt::preempt::SOFTIRQ_MASK) == crate::interrupt::preempt::SOFTIRQ_OFFSET
        {
            crate::debug::leakorigin::note_softirq_release(self as *const Task as *mut Task);
            self.ti_pc_sirq_present_run.store(
                0,
                core::sync::atomic::Ordering::Relaxed,
            );
        }
        let after = prev.wrapping_sub(val);
        if after < 0 {
            self.ti_preempt_count.store(0, core::sync::atomic::Ordering::Release);
            // Post-clamp presence is new presence — re-arm all bars so
            // the corrupted section's successor guards are not adjudged
            // against the pre-clamp run.
            self.ti_pc_present_runs_reset_all();
            preempt_underflow_report(self.pid(), prev, val, loc, 0);
        }
    }

    /// Read the pid of a task pointer WITHOUT dereferencing it in the
    /// caller's context (the 119th IRQ-bracket release names the
    /// booked task; the pointer may be a freed page, so the read is
    /// guarded by the refcount check inside the release helper).
    pub fn task_pid_of(task: *mut Task) -> u32 {
        if task.is_null() {
            return 0;
        }
        // SAFETY: the pointer came from the current task at irq_enter;
        // the read is a single u32 load and the value is only a
        // diagnostic label.
        crate::process::r#unsafe::borrow_task(task).pid()
    }

    /// Release a hardirq base booked on `task` from a context where the
    /// current task is a different one (the 119th epoll-pwait
    /// conviction: a guard's release must act on the task that took
    /// it). Returns false when the task page is already freed (the
    /// write is then skipped — a freed task's count is meaningless).
    pub fn release_hardirq_base_on(
        task: *mut Task,
        val: i32,
        loc: &'static core::panic::Location<'static>,
    ) -> bool {
        if task.is_null() {
            return false;
        }
        // The pointer was a live task at irq_enter; the refcount check
        // inside the method rejects a freed page (INV-PROC-BORROW-1).
        crate::process::r#unsafe::borrow_task_mut(task).release_hardirq_base_remote(val, loc)
    }

    /// Release a HARDIRQ base that was booked on THIS task by
    /// `irq_enter`, from a context where the current task is a DIFFERENT
    /// one (the bracket spanned a context switch — the 119th
    /// epoll-pwait conviction: the release used to land on whatever
    /// task was current at irq_exit, stranding the 0x10000 base on the
    /// interrupted task forever and underflowing the incoming one).
    ///
    /// SAFE: the liveness check rejects a freed page (the refcount is
    /// zero once the task was freed) — the count of a freed task is
    /// meaningless, so skipping it is correct.
    pub fn release_hardirq_base_remote(
        &self,
        val: i32,
        loc: &'static core::panic::Location<'static>,
    ) -> bool {
        if self.task_refcnt.load(core::sync::atomic::Ordering::Acquire) == 0 {
            return false;
        }
        self.sub_preempt_count_at(val, loc);
        true
    }

    /// As `sub_preempt_count_at` with the guard's lock instance for the
    /// underflow report (0 = caller had no lock to name).
    #[inline]
    pub fn sub_preempt_count2_at(
        &self,
        val: i32,
        loc: &'static core::panic::Location<'static>,
        site: usize,
    ) {
        let prev = self.ti_preempt_count.fetch_sub(val, core::sync::atomic::Ordering::Release);
        if val == crate::interrupt::preempt::PREEMPT_OFFSET
            && (prev & crate::interrupt::preempt::PREEMPT_MASK) == 1
        {
            crate::debug::leakorigin::note_release(self as *const Task as *mut Task);
            // phantom-bits / stress-port r3: live pair completed — re-arm
            // the run counter the heal gates on (see sub_preempt_count_at;
            // the old store hit the superseded ti_pc_zero_jif field).
            self.ti_pc_pirq_present_run.store(
                0,
                core::sync::atomic::Ordering::Relaxed,
            );
        }
        let after = prev.wrapping_sub(val);
        if after < 0 {
            self.ti_preempt_count.store(0, core::sync::atomic::Ordering::Release);
            self.ti_pc_present_runs_reset_all();
            preempt_underflow_report(self.pid(), prev, val, loc, site);
        }
    }

    /// R74: zero a corrupted (negative) preempt_count — the self-heal for
    /// the double-run engine's unpaired-decrement fingerprint. A plain
    /// store (not fetch) is intended: whatever the count was, it was
    /// garbage.
    #[inline]
    pub fn ti_preempt_count_zero(&self) {
        self.ti_preempt_count.store(0, core::sync::atomic::Ordering::Release);
        // stress-port r3: every external zeroing re-arms the heal bars —
        // presence observed after the zero is new presence (otherwise
        // the stale bar re-heals each live guard: the held_jiffies=0
        // post-heal storm family).
        self.ti_pc_present_runs_reset_all();
    }

    /// Stress-fleet session 3 (2026-10-25): clear ONLY the task-context
    /// base bits (PREEMPT + SOFTIRQ), preserving HARDIRQ and NMI nesting.
    ///
    /// The tick-side PREEMPT-HEAL runs inside the timer IRQ bracket, so
    /// the observed task's count ALWAYS carries the tick's own in-flight
    /// HARDIRQ_OFFSET at heal time. The old whole-word zero deleted that
    /// bit too, and the irq_exit sub a few frames up then landed on a
    /// zeroed count — one guaranteed PREEMPT-UNDERFLOW (sub=0x10000,
    /// count_was=0) per heal firing. Fleet correlation (ltp-a r18-r20 +
    /// memstorm r9/r10): pid-1 HEAL 31x against UNDERFLOW 40x, with the
    /// underflow report landing at preempt.rs's irq_exit sub. Keeping the
    /// interrupt-nesting bits lets the still-pending irq_exit/nmi_exit
    /// subs pair exactly; only the phantom guard bits the heal adjudicated
    /// (base != 0 held 10 s) are dropped.
    #[inline]
    pub fn ti_preempt_count_clear_base(&self) {
        use core::sync::atomic::Ordering;
        self.ti_preempt_count.fetch_and(
            crate::interrupt::preempt::HARDIRQ_MASK
                | crate::interrupt::preempt::NMI_MASK,
            Ordering::Release,
        );
        // stress-port r3: the fields this heal clears get fresh bars.
        self.ti_pc_present_runs_reset_base();
    }

    /// Tick-side PREEMPT-HEAL, unified form: keep exactly ONE
    /// HARDIRQ_OFFSET (the observer's own in-flight timer IRQ level,
    /// so the still-pending irq_exit sub pairs) plus NMI nesting, and
    /// drop everything else — phantom base bits AND phantom extra
    /// hardirq nesting (leg-19 round-3e pure-hardirq phantom,
    /// base == 0 with hardirq extra held 10 s). When there is no
    /// extra hardirq nesting the result is bit-identical to
    /// `ti_preempt_count_clear_base`.
    #[inline]
    pub fn ti_preempt_count_keep_tick_nesting(&self) {
        use core::sync::atomic::Ordering;
        self.ti_preempt_count.fetch_and(
            crate::interrupt::preempt::HARDIRQ_OFFSET
                | crate::interrupt::preempt::NMI_MASK,
            Ordering::Release,
        );
        // stress-port r3: this form clears base AND extra hardirq
        // levels — all three fields get fresh bars (the kept single
        // level is the observer tick's own, whose pending irq_exit sub
        // is not judged by these runs anyway).
        self.ti_pc_present_runs_reset_all();
    }

    /// Get kernel stack pointer (thread_info.kernel_sp)
    #[inline]
    pub fn ti_kernel_sp(&self) -> u64 {
        self.ti_kernel_sp.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Set kernel stack pointer (thread_info.kernel_sp)
    #[inline]
    pub fn set_ti_kernel_sp(&self, sp: u64) {
        self.ti_kernel_sp.store(sp, core::sync::atomic::Ordering::Release);
    }

    /// Get user stack pointer (thread_info.user_sp)
    #[inline]
    pub fn ti_user_sp(&self) -> u64 {
        self.ti_user_sp.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Set user stack pointer (thread_info.user_sp)
    #[inline]
    pub fn set_ti_user_sp(&self, sp: u64) {
        self.ti_user_sp.store(sp, core::sync::atomic::Ordering::Release);
    }

    /// Get running CPU (thread_info.cpu)
    #[inline]
    pub fn ti_cpu(&self) -> i32 {
        self.ti_cpu.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// LEGACY (superseded 2026-11-02, stress-port r3): the single-field
    /// drain stamp. The pb3 release-side re-arm stored THIS field while
    /// the heal gated on the per-field stamps — the re-arm never armed
    /// the bar that mattered (see ti_pc_*_present_run docs). Kept for
    /// layout stability; stores removed; no readers.
    #[inline]
    pub fn ti_pc_zero_jif(&self) -> u64 {
        self.ti_pc_zero_jif.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Current consecutive-present run for the SOFTIRQ field (see the
    /// field docs; read by scheduler_tick's heal gate).
    #[inline]
    pub fn ti_pc_sirq_present_run(&self) -> u64 {
        self.ti_pc_sirq_present_run.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Current consecutive-present run for the HARDIRQ field.
    #[inline]
    pub fn ti_pc_hirq_present_run(&self) -> u64 {
        self.ti_pc_hirq_present_run.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// Current consecutive-present run for the base PREEMPT field.
    #[inline]
    pub fn ti_pc_pirq_present_run(&self) -> u64 {
        self.ti_pc_pirq_present_run.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// One tick-side observation of all three fields (caller:
    /// scheduler_tick on the task's own CPU — the only bump writer).
    /// Present fields extend their run; absent fields re-arm to 0.
    /// `h_extra` is the extra-hardirq-nesting predicate (>= 2 levels),
    /// so the observer tick's own in-flight level never counts.
    #[inline]
    pub fn ti_pc_present_runs_tick(&self, p_set: bool, s_set: bool, h_extra: bool) {
        use core::sync::atomic::Ordering;
        if p_set {
            self.ti_pc_pirq_present_run.fetch_add(1, Ordering::Relaxed);
        } else {
            self.ti_pc_pirq_present_run.store(0, Ordering::Relaxed);
        }
        if s_set {
            self.ti_pc_sirq_present_run.fetch_add(1, Ordering::Relaxed);
        } else {
            self.ti_pc_sirq_present_run.store(0, Ordering::Relaxed);
        }
        if h_extra {
            self.ti_pc_hirq_present_run.fetch_add(1, Ordering::Relaxed);
        } else {
            self.ti_pc_hirq_present_run.store(0, Ordering::Relaxed);
        }
    }

    /// Re-arm the PREEMPT + SOFTIRQ run counters (called by the count
    /// zeroing/heal methods that clear exactly those fields). Presence
    /// observed AFTER an external zeroing is new presence and deserves
    /// a fresh 200-observation window — the stress-port r2 boots showed
    /// the opposite: the first (correct) heal of a parked phantom left
    /// the stale bar in place, and every later tick that sampled the
    /// task inside ANY live guard re-healed instantly (held_jiffies=0
    /// storm, 17-30 fires per episode), each fire stripping a live
    /// guard's bit (the spinlock.rs:0x1b8 underflow family at the
    /// page-allocator zone lock).
    #[inline]
    fn ti_pc_present_runs_reset_base(&self) {
        use core::sync::atomic::Ordering;
        self.ti_pc_pirq_present_run.store(0, Ordering::Relaxed);
        self.ti_pc_sirq_present_run.store(0, Ordering::Relaxed);
    }

    /// Re-arm all three run counters (called by whole-word zeroings —
    /// parked-heal, exit-path clears, underflow clamps).
    #[inline]
    fn ti_pc_present_runs_reset_all(&self) {
        use core::sync::atomic::Ordering;
        self.ti_pc_pirq_present_run.store(0, Ordering::Relaxed);
        self.ti_pc_sirq_present_run.store(0, Ordering::Relaxed);
        self.ti_pc_hirq_present_run.store(0, Ordering::Relaxed);
    }

    /// Set running CPU (thread_info.cpu)
    #[inline]
    pub fn set_ti_cpu(&self, cpu: i32) {
        self.ti_cpu.store(cpu, core::sync::atomic::Ordering::Release);
    }

    /// CPU affinity mask (thread_info.cpus_allowed).
    /// Bit N set means CPU N is allowed.  All bits set = all CPUs allowed.
    #[inline]
    pub fn cpus_allowed(&self) -> u32 {
        self.cpus_allowed.load(core::sync::atomic::Ordering::Acquire)
    }

    /// Default affinity for a NEWLY-CREATED task: the mask of CPUs that are
    /// ONLINE at creation time (Linux: a fresh task inherits the parent's
    /// cpus_mask, and the boot-time default is the platform's possible set —
    /// which on a real machine equals the -smp CPU count, never the
    /// compile-time MAX_CPUS ceiling). Initializing with !0 reported CPUs
    /// that can never come online (MAX_CPUS=4 on a -smp 2 boot): LTP
    /// getcpu01/nice05 take the highest bit from sched_getaffinity and pass
    /// it back to sched_setaffinity, which correctly rejects never-online
    /// CPUs with EINVAL — the kernel's own view was self-inconsistent.
    /// Tasks created before SMP bring-up keep the boot-CPU mask (Linux
    /// early kthreads behave the same way); every userspace task is created
    /// after start_secondaries() and gets the full online set.
    pub fn default_cpus_allowed() -> u32 {
        let online = crate::arch::smp::num_started_cpus().max(1);
        if online >= 32 {
            u32::MAX
        } else {
            (1u32 << online) - 1
        }
    }

    /// Widen the masks of tasks created before SMP bring-up to the now-known
    /// online set (the Linux sched_cpu_activate/cpuset refresh analogue).
    /// Called from start_secondaries() once the secondaries are online: at
    /// that point only kernel-boot tasks and the boot init task exist, none
    /// of them carrying a USER-requested affinity, so overwriting the
    /// boot-default masks ({CPU0} from the pre-SMP default, or !0 from the
    /// idle-task initializer) with the online mask cannot clobber an
    /// explicit sched_setaffinity. Without this, the boot init task (pid 1,
    /// created before start_secondaries) kept a CPU0-only mask on every
    /// -smp>1 boot and every userspace process inherited it.
    pub fn refresh_default_affinity_to_online() {
        let online = Self::default_cpus_allowed();
        let mut visited = 0usize;
        let mut widened = 0usize;
        crate::sched::hotpath::for_each_task(|task| {
            // for_each_task yields live &Task borrows (walk-pinned).
            let cur = task.cpus_allowed.load(core::sync::atomic::Ordering::Relaxed);
            if cur == 1 || cur == u32::MAX {
                task.cpus_allowed
                    .store(online, core::sync::atomic::Ordering::Release);
                widened += 1;
            }
            visited += 1;
        });
        crate::pr_info!(
            "affinity: {} of {} task(s) default mask -> {:#x}",
            widened,
            visited,
            online
        );
    }

    /// R12-5: drop a lookup pin; the LAST put frees the slot.
    #[inline]
    pub fn task_put(task: *mut Task) {
        if task.is_null() { return; }
        // The putter's pin discipline (R12-5): the caller's reference
        // keeps the page alive through this decrement
        // (INV-PROC-BORROW-1).
        let prev = crate::process::r#unsafe::borrow_task(task)
            .task_refcnt
            .fetch_sub(1, core::sync::atomic::Ordering::AcqRel);
        if prev == 1 {
            crate::sched::hotpath::free_task_slot(task);
        }
    }

    /// R56-heal (lifecycle pin): try to acquire a reference on a task that
    /// the caller holds VALID ONLY UNDER THE PID-HASH BUCKET LOCK. The
    /// harvest scan dereferences (and its heals WRITE) task pages far
    /// beyond the bucket-lock section — an exiting task can complete
    /// pid_hash_remove + synchronize_rcu + task_put in that window and the
    /// page returns to the (non-zeroing) allocator, where fork's
    /// new_task_at immediately reinitializes it: the m25 adjudication
    /// capture (pb_7) caught exactly that — the steal's pre-revoke read
    /// saw PICK_LIVE and microseconds later its CAS(PICKED → REVOKED)
    /// won because fork had rewritten ti_pick_state on the recycled page;
    /// the heal then linked the half-built child (queue corruption,
    /// epc=0x0 double panic). A taken pin (same discipline as
    /// collect_children_pinned / E8-REPAR) defers free_task_slot until
    /// the healer's own task_put, and the heals' state re-checks refuse a
    /// task that exited under the pin. Returns false when the page is at
    /// (or past) its last reference — treat as not healable.
    pub fn try_pin(task: *mut Task) -> bool {
        if task.is_null() {
            return false;
        }
        {
            let prev = crate::process::r#unsafe::borrow_task(task)
                .task_refcnt
                .fetch_add(1, core::sync::atomic::Ordering::AcqRel);
            if prev == 0 {
                // Freed (or hash-contract-broken) page: undo our own
                // increment so a recycled owner is not left with a
                // permanent extra reference, and refuse the pin.
                crate::process::r#unsafe::borrow_task(task)
                    .task_refcnt
                    .fetch_sub(1, core::sync::atomic::Ordering::AcqRel);
                return false;
            }
            true
        }
    }

    /// R8-1b: on-CPU flag — set at pick time, cleared by __switch_to after
    /// this task's context is saved. Other CPUs' picks skip on-CPU tasks;
    /// the CPU switching the task out may re-pick itself (no restore).
    #[inline]
    pub fn on_cpu(&self) -> bool {
        self.ti_on_cpu.load(core::sync::atomic::Ordering::Acquire)
    }

    #[inline]
    pub fn set_on_cpu(&self, v: bool) {
        self.ti_on_cpu.store(v, core::sync::atomic::Ordering::Release)
    }

    /// R56-heal (pick ownership publish): write the FULL ownership qword
    /// {ti_on_cpu = true, running_on_cpu = cpu} with one aligned 8-byte
    /// store — the exact reverse of __switch_to's switch-out release
    /// (0xFFFFFFFF_00000000) — and reset ti_pick_state for the new pick.
    /// The claim previously appeared only at switch-in (the Rust wrapper /
    /// x86 publish), leaving the whole pick → __switch_to window
    /// claim-less: the slot-capture harvester could revoke a pick that a
    /// TCG-starved (but perfectly live) vCPU was still about to resume,
    /// and nothing at the resume point could detect the revocation.
    /// Publishing the claim at pick time (under the GRQ lock, where no
    /// other picker can observe the task) gives the switch-in a
    /// validation point. The authoritative arbitration, however, is
    /// ti_pick_state: `live` distinguishes a fresh pick (PICKED — the
    /// resume path must CAS it LIVE before restoring registers) from the
    /// next == prev fast-path repick (the task never stopped running —
    /// publish LIVE directly).
    #[inline]
    pub fn publish_pick_ownership(&self, cpu: i32, live: bool) {
        // SAFETY: ti_on_cpu is 8-byte aligned and running_on_cpu sits at
        // +4 inside the same qword (asserted above); the overlaying
        // AtomicU64 store keeps the pair consistent for the non-atomic
        // pick-side readers.
        crate::process::r#unsafe::store_pick_qword(
            &self.ti_on_cpu as *const _ as usize,
            ((cpu as u32 as u64) << 32) | 1,
        );
        self.ti_pick_state.store(
            if live { PICK_LIVE } else { PICK_PICKED },
            core::sync::atomic::Ordering::Release,
        );
    }

    /// R56-heal: release both ownership bits with the single aligned
    /// store the switch-out asm uses ({on_cpu = false, claim = -1}).
    /// Healer-side twin of publish_pick_ownership.
    #[inline]
    pub fn release_pick_ownership(&self) {
        crate::process::r#unsafe::store_pick_qword(
            &self.ti_on_cpu as *const _ as usize,
            0xFFFF_FFFF_0000_0000,
        );
    }

    /// R56-heal (resume-side affirm): atomically PICKED → LIVE. Called by
    /// the switch-in (__switch_to / x86_switch_publish) as the LAST step
    /// before the task's registers are restored — winning this CAS is the
    /// exclusive right to resume the pick. Ok(()) = affirmed (resume);
    /// Err(observed) = PICK_REVOKED (the harvester stole the pick —
    /// abort the resume) or a protocol breach (report and abort).
    #[inline]
    pub fn try_affirm_pick(&self) -> Result<(), u32> {
        match self.ti_pick_state.compare_exchange(
            PICK_PICKED,
            PICK_LIVE,
            core::sync::atomic::Ordering::AcqRel,
            core::sync::atomic::Ordering::Acquire,
        ) {
            Ok(_) => {
                crate::sched::hotpath::pick_trace(self as *const Task as *mut Task, 6);
                // Wakewatch (rux:schedtrace (retired debug=schedtrace)): the switch-in affirm — the
                // pick materialized into a register restore.
                crate::sched::hotpath::ww_record(
                    self as *const Task as *mut Task,
                    crate::api::sched::WW_EV_AFFIRM,
                );
                // pick→switch ledger (wake-fastrepro): the pick
                // terminated — consume its publication record.
                crate::sched::hotpath::psledger_consume(
                    self as *const Task as *mut Task,
                );
                Ok(())
            }
            Err(observed) => {
                crate::sched::hotpath::pick_trace(self as *const Task as *mut Task, 7);
                Err(observed)
            }
        }
    }

    /// R56-heal (pick state read) — for the wrapper's early check and the
    /// harvester's pre-filter.
    #[inline]
    pub fn pick_state(&self) -> u32 {
        self.ti_pick_state.load(core::sync::atomic::Ordering::Acquire)
    }

    /// R56-heal (healer-side revocation): atomically PICKED → REVOKED.
    /// Called by the harvester under the GRQ. Returns false when the pick
    /// must NOT be healed: PICK_LIVE (the task genuinely runs on its
    /// picking CPU — the m25 lesson: tick staleness under MTTCG proves
    /// nothing) or an already-consumed REVOKED marker.
    #[inline]
    pub fn revoke_pending_pick(&self) -> bool {
        matches!(
            self.ti_pick_state.compare_exchange(
                PICK_PICKED,
                PICK_REVOKED,
                core::sync::atomic::Ordering::AcqRel,
                core::sync::atomic::Ordering::Acquire,
            ),
            Ok(_)
        )
    }

    /// R65-pick (probe-slow): dual-bit ownership test for the class picks,
    /// read from the ownership qword in ONE naturally-aligned load.
    ///
    /// `ti_on_cpu` (1 byte) and `running_on_cpu` (i32 at +4) share one
    /// 8-byte word (layout pinned by the asserts in
    /// arch/riscv64/context.rs). The switch-out release writes the whole
    /// word in a single `sd`; the pick-time mark and the switch-in claim
    /// write the halves separately. A single volatile u64 load observes a
    /// same-instant snapshot of both halves — a strict SUBSET of the
    /// (earlier-byte, later-claim) combinations the old byte-load +
    /// Acquire-atomic-load pair could observe, so every state the old
    /// predicate handled correctly is handled identically, with one load
    /// and no acquire fence per candidate instead of two loads. The
    /// semantics are exactly e87a8b40's: pickable only when
    /// on_cpu == false AND claim == -1 (or the picker's own claim).
    #[inline]
    pub fn ownership_pickable(&self, picker: usize) -> bool {
        const _: () = {
            assert!(core::mem::offset_of!(Task, ti_on_cpu) % 8 == 0);
            assert!(
                core::mem::offset_of!(Task, running_on_cpu)
                    == core::mem::offset_of!(Task, ti_on_cpu) + 4
            );
        };
        // Little-endian decode: byte at +0 is the low byte, i32 at +4 the
        // high half. Both supported arches (riscv64, x86_64) are LE; the
        // cfg gate keeps a BE port from silently misdecoding.
        #[cfg(not(target_endian = "little"))]
        compile_error!("ownership_pickable assumes little-endian qword decode");

        let word = crate::process::r#unsafe::read_volatile_u64(
            core::ptr::addr_of!(self.ti_on_cpu) as *const u64,
        );
        let oncpu = (word & 0xFF) != 0;
        let claim = (word >> 32) as i32;
        !oncpu && (claim == -1 || claim as usize == picker)
    }

    /// Set CPU affinity mask.
    #[inline]
    pub fn set_cpus_allowed(&self, mask: u32) {
        self.cpus_allowed.store(mask, core::sync::atomic::Ordering::Release);
    }

    /// Check if a specific CPU is allowed for this task.
    #[inline]
    pub fn cpu_allowed(&self, cpu: usize) -> bool {
        let mask = if cpu < 32 { 1u32 << cpu } else { 0u32 };
        self.cpus_allowed.load(core::sync::atomic::Ordering::Acquire) & mask != 0
    }

    /// Preferred wake CPU for this task (wake-affine hint).
    /// Returns -1 when no hint is set.
    #[inline]
    pub fn wake_cpu(&self) -> i32 {
        self.ti_cpu.load(core::sync::atomic::Ordering::Relaxed)
    }

    // ==================== Kernel stack management ====================

    /// Allocate kernel stack
    ///
    ///
    /// Allocates a kernel stack for current task, size is KERNEL_STACK_SIZE (16KB)
    ///
    /// # Returns
    /// Some(stack top address) on success, None on failure
    pub fn alloc_kernel_stack(&mut self) -> Option<*mut u8> {
        // Use stack cache for allocation
        let stack_ptr = stack_cache_alloc();

        if !stack_ptr.is_null() {
            // Set stack top address (stack grows downward): stack_ptr
            // was returned by stack_cache_alloc() with exactly
            // KERNEL_STACK_SIZE bytes allocated.
            let stack_top = crate::process::r#unsafe::slot_ptr(stack_ptr, KERNEL_STACK_SIZE);
            self.kernel_stack = Some(stack_top);

            // Save stack bottom for overflow detection
            self.kernel_stack_bottom = stack_ptr as usize;

            // Also set ti_kernel_sp
            self.set_ti_kernel_sp(stack_top as u64);

            Some(stack_top)
        } else {
            None
        }
    }

    /// Free kernel stack
    ///
    ///
    /// Free current task's kernel stack (returns to cache)
    pub fn free_kernel_stack(&mut self) {
        if let Some(stack_top) = self.kernel_stack {
            // Calculate stack bottom address (stack top - stack size):
            // kernel_stack was set by alloc_kernel_stack() as bottom +
            // KERNEL_STACK_SIZE, so subtracting KERNEL_STACK_SIZE yields
            // the original allocation base.
            let stack_bottom = stack_top.wrapping_sub(KERNEL_STACK_SIZE);

            // R13-6: the pop path ZEROES 32KB — a corrupted kernel_stack
            // field would wipe arbitrary heap pages (linked Task pages
            // read as pid=0/state=0 afterwards; the zeroed-children-node
            // family). Validate alignment and heap bounds before pushing;
            // a bogus value is reported and dropped (leak beats wipe).
            {
                use crate::console::putchar;
                let addr = stack_bottom as usize;
                let (hs, he) = crate::mm::hotpath::GLOBAL_ALLOCATOR.heap_bounds();
                let bogus = addr & 0xF != 0
                    || addr + KERNEL_STACK_SIZE > he
                    || addr < hs;
                if bogus {
                    const MSG: &[u8] = b"stackfree: BOGUS bottom=0x";
                    for &b in MSG { putchar(b); }
                    let mut sh = 64;
                    while sh > 0 {
                        sh -= 4;
                        let nb = ((addr >> sh) & 0xF) as u8;
                        putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
                    }
                    putchar(b'\n');
                    self.kernel_stack = None;
                    return;
                }
            }

            // Leg-16 anatomy (rux:schb (retired debug=schb),ksprobe): high-water scan + record
            // stash. The stack is dead (on_cpu cleared + RCU waited
            // upstream), no locks held — safe to walk. Records stash the
            // surviving return addresses for the next schbd beat (the
            // in-context print was eaten by early-boot UART storms twice).
            if crate::trace::EVENT_SCHB.armed()
                && crate::trace::EVENT_KSPROBE.armed()
            {
                let base = stack_bottom as usize;
                let mut off = 8usize; // skip the canary slot
                while off < KERNEL_STACK_SIZE {
                    // Bounds validated above (base..base+SIZE).
                    let v = crate::process::r#unsafe::read_volatile_u64((base + off) as *const u64);
                    if v != 0 {
                        let hw = (KERNEL_STACK_SIZE - off) as u64;
                        let old = KSTACK_HW.load(Ordering::Relaxed);
                        if hw > old {
                            KSTACK_HW.store(hw, Ordering::Relaxed);
                            if hw > 40 * 1024 {
                                use crate::console::putchar;
                                const MSG: &[u8] = b"KSTACK-HW depth=";
                                for &b in MSG { putchar(b); }
                                let mut vv = hw;
                                if vv == 0 { putchar(b'0'); }
                                let mut dd = [0u8; 20]; let mut kk = 0;
                                while vv > 0 { dd[kk] = b'0' + (vv % 10) as u8; kk += 1; vv /= 10; }
                                while kk > 0 { kk -= 1; putchar(dd[kk]); }
                                const P: &[u8] = b" pid=";
                                for &b in P { putchar(b); }
                                let mut pv = self.pid() as usize;
                                if pv == 0 { putchar(b'0'); }
                                let mut pd = [0u8; 10]; let mut pk = 0;
                                while pv > 0 { pd[pk] = b'0' + (pv % 10) as u8; pk += 1; pv /= 10; }
                                while pk > 0 { pk -= 1; putchar(pd[pk]); }
                                const C: &[u8] = b" comm='";
                                for &b in C { putchar(b); }
                                for &cb in self.comm() { if cb != 0 { putchar(cb); } }
                                // sc218 lesson: the region BELOW the first
                                // nonzero word is a zeroed span (a memset'd
                                // local or zero-initialised frames) — by
                                // construction it carries no return
                                // addresses. The surviving frames of the
                                // deep chain sit just ABOVE `off`; walk
                                // upward and print the first kernel-range
                                // words (deepest surviving return
                                // addresses, symbolisable with addr2line).
                                const T: &[u8] = b"' tail:";
                                for &b in T { putchar(b); }
                                let mut shown = 0;
                                let mut o2 = off;
                                let o_end = off + 4096;
                                while o2 < o_end && o2 < KERNEL_STACK_SIZE && shown < 40 {
                                    // In-bounds per loop guard.
                                    let ra = crate::process::r#unsafe::read_volatile_u64((base + o2) as *const u64);
                                    if ra >= 0xffffffff_80000000u64 && ra <= 0xffff_ffff_ffff_f000u64 {
                                        putchar(b' ');
                                        let mut sh2 = 64;
                                        while sh2 > 0 {
                                            sh2 -= 4;
                                            let nb = ((ra >> sh2) & 0xF) as u8;
                                            putchar(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
                                        }
                                        shown += 1;
                                    }
                                    o2 += 8;
                                }
                                putchar(b'\n');
                            }
                        }
                        break;
                    }
                    off += 8;
                }
            }

            // DEBUG (LTP chunk@700 panic hunt, 2026-11-02) + R87 CLOSURE
            // (oh-launch round 10): if any CPU's live TSS.rsp0 still
            // points at THIS stack's top, every later user-origin trap
            // entry on that CPU pushes its frame into freed-and-recycled
            // memory — the stale-rsp0 engine behind the deterministic
            // rip=0 "Kernel stack overflow" panics (frames observed ~7KB
            // deep inside an unowned kstack slot), and the LIVE producer
            // of the recycled-stack foreign-content writes: fr3 caught it
            // in the act (STACKFREE-LIVE-RSP0 cpu=3, an aborted switch
            // leaves the dying task's top installed, the stack freed and
            // recycled, the next trap frame / re-carve tearing the new
            // owner — user-range pt_regs values in kernel frames, the
            // #GP-on-ret family at 1.2-1.5 s on every armed boot).
            // x86_update_rsp0 has TWO silent keep-the-old-value legs
            // (top==0, out-of-heap-bound — the idle task's static boot
            // stack fails the bound), and the abort path skips the rsp0
            // install entirely — so the free side must be the closure:
            // NEVER hand the stack to the cache while a live rsp0 names
            // it. Bounded wait (the next switch-in on that CPU installs
            // its own top, microseconds normally); past the budget the
            // stack is WITHHELD — leaked from the cache forever (a trap
            // frame pushed at a leaked stack writes into memory nobody
            // will ever own) — never recycled under a live rsp0. The old
            // detector here was pr_debug (the campaign's fifth blind
            // printer — invisible at LOGLEVEL=4); it now reports through
            // the raw-uart taskdump channel, bounded, on the withhold
            // only.
            #[cfg(feature = "x86_64")]
            let recycle_ok = {
                const SFR_RSP0_WAIT_JIFFIES: u64 = 3;
                let sfr_armed_at = crate::timer::get_jiffies();
                loop {
                    let mut stale_cpu: usize = usize::MAX;
                    for c in 0..crate::config::MAX_CPUS {
                        let rsp0 = crate::arch::x86_64::smp::PER_CPU[c]
                            .tss_rsp0
                            .load(core::sync::atomic::Ordering::Acquire);
                        if rsp0 == stack_top as u64 {
                            stale_cpu = c;
                            break;
                        }
                    }
                    if stale_cpu == usize::MAX {
                        break true;
                    }
                    if crate::timer::get_jiffies()
                        .wrapping_sub(sfr_armed_at)
                        >= SFR_RSP0_WAIT_JIFFIES
                    {
                        static RSP0_SEEN: core::sync::atomic::AtomicU32 =
                            core::sync::atomic::AtomicU32::new(0);
                        let n = RSP0_SEEN
                            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                        if n < 64 {
                            crate::debug::taskdump::taskdump_raw_line(
                                b"STACKFREE-LIVE-RSP0 cpu=",
                            );
                            crate::debug::taskdump::taskdump_dec(stale_cpu as u64);
                            crate::debug::taskdump::taskdump_raw_line(b" top=0x");
                            let mut v = stack_top as u64;
                            let mut sh: i32 = 64;
                            while sh > 0 {
                                sh -= 4;
                                let nb = ((v >> sh) & 0xF) as u8;
                                crate::debug::taskdump::taskdump_raw_line(&[(if nb < 10 {
                                    b'0' + nb
                                } else {
                                    b'a' + nb - 10
                                })]);
                            }
                            crate::debug::taskdump::taskdump_raw_line(b" freeing_pid=");
                            crate::debug::taskdump::taskdump_dec(self.pid() as u64);
                            crate::debug::taskdump::taskdump_raw_line(
                                b" (recycle WITHHELD - leaked, never re-carved)\n",
                            );
                        }
                        break false;
                    }
                    core::hint::spin_loop();
                }
            };
            #[cfg(not(feature = "x86_64"))]
            let recycle_ok = true;

            // DEBUG: record for the freed-stack execution detector.
            note_kstack_freed(
                stack_bottom as usize,
                stack_top as usize,
                self.pid(),
            );

            // SCRIBBLE TRAP (oh-launch round 9): paint the whole freed
            // stack with the poison byte — the slab still recycles freed
            // stacks, and the early-free race (the dying task executing
            // its final frames while the slot is already handed onward —
            // the disabled-cache-reuse comment's own fingerprint: a user
            // sret restoring a pt_regs written by the NEW owner) predicts
            // writes into freed stacks. The alloc-side scan names any
            // non-poison word WITH its previous owner. Armed boots only
            // (rux:scribble); the paint is the same cost class as the
            // leg-16 zeroing.
            // R87 (instrument correction): skip the canary qword at the
            // bottom — the round-9 paint covered it, so EVERY armed-boot
            // free tripped stack_cache_free's canary check below and
            // printed a false STACK-OVERFLOW (fr1: canary read back as
            // the paint value 0xa5a5...), burying the real overflow
            // signal under one artifact line per free.
            // R87 (rsp0 closure): a withheld stack (live rsp0 naming it)
            // is never recycled — no paint, no cache return.
            if recycle_ok {
                if crate::trace::EVENT_SCRIBBLE.armed() {
                    crate::process::r#unsafe::paint_bytes(
                        stack_bottom,
                        8,
                        0xA5,
                        KERNEL_STACK_SIZE - 8,
                    );
                }

                // Return to cache instead of deallocating
                stack_cache_free(stack_bottom);
            }

            // Clear reference
            self.kernel_stack = None;
            self.kernel_stack_bottom = 0;
            // Clear ti_kernel_sp
            self.set_ti_kernel_sp(0);
        }
    }

    /// Get kernel stack top address
    ///
    /// Used to set SP register during context switch
    pub fn get_kernel_stack(&self) -> Option<*mut u8> {
        self.kernel_stack
    }

    /// Get kernel stack bottom address (for overflow detection)
    #[inline]
    pub fn kernel_stack_bottom(&self) -> usize {
        self.kernel_stack_bottom
    }

    /// Get kernel stack top address (highest address + 1)
    #[inline]
    pub fn kernel_stack_top(&self) -> usize {
        self.kernel_stack_bottom + KERNEL_STACK_SIZE
    }

    /// sc261 (leg-23): a thread's kernel stack is returned to the
    /// stack cache and kernel_stack_bottom zeroed at exit time (see
    /// free_kernel_stack), while the Task struct itself survives until
    /// its last pin drops — kernel_stack_top() then reports
    /// KERNEL_STACK_SIZE and every entry-frame reader computes a small
    /// bogus address. sc261: kill_dump_tgid dereferenced 0x13fd0
    /// (= 0x14000 - PT_REGS_SIZE + offsetof(PtRegs, orig_rax)) on an
    /// exited-but-pinned thread and took the boot down with an
    /// unfixable kernel fault inside the SIGKILL teardown
    /// (rip = <u64 as LowerHex>::fmt entry, load of the format value,
    /// pid 1421). Foreign-task entry-frame diagnostics MUST check this
    /// first; false means the thread has exited and its pt_regs frame
    /// is gone. (A task's own current thread always passes.)
    #[inline]
    pub fn owns_kernel_stack(&self) -> bool {
        self.kernel_stack_bottom != 0
    }

    /// Diagnostic-safe kernel stack top (KILLDUMP census family).
    ///
    /// free_kernel_stack() clears kernel_stack_bottom to 0, but the Task
    /// itself stays alive (pinned / unreaped) and the KILLDUMP and wedge
    /// censuses walk FOREIGN tasks that may have exited CONCURRENTLY:
    /// their thread-list snapshot can contain a task whose stack is
    /// already freed. kernel_stack_top() then "returns" 0 + KERNEL_STACK
    /// _SIZE (0x14000 on x86_64) and the census's entry-frame read
    /// faults at top-PT_REGS_SIZE+off (badaddr 0x13ff0/0x13ff8, monkey-b
    /// r38 + monkey-a r32 — identical on v9 and v10, deterministic),
    /// taking the whole boot down from inside a PRINTER. Same shape for
    /// a task collected between Task creation and alloc_kernel_stack.
    ///
    /// Every diagnostic reader of a FOREIGN task's stack must use this
    /// and skip on None. The scheduler/context-switch paths keep the raw
    /// kernel_stack_top(): they only ever run on tasks that own a live
    /// stack.
    #[inline]
    pub fn kernel_stack_top_checked(&self) -> Option<usize> {
        if self.kernel_stack_bottom == 0 {
            return None;
        }
        Some(self.kernel_stack_bottom + KERNEL_STACK_SIZE)
    }

    /// Check if address is within kernel stack range (for overflow detection)
    ///
    /// # Arguments
    /// - `addr`: Address to check
    ///
    /// # Returns
    /// true if address is within kernel stack range
    #[inline]
    pub fn is_in_kernel_stack(&self, addr: usize) -> bool {
        if self.kernel_stack_bottom == 0{
            return false;
        }
        let stack_top = self.kernel_stack_bottom + KERNEL_STACK_SIZE;
        addr >= self.kernel_stack_bottom && addr < stack_top
    }

    /// Check if stack pointer indicates potential overflow
    ///
    /// # Arguments
    /// - `sp`: Stack pointer value to check
    ///
    /// # Returns
    /// true if SP is below stack bottom (overflow detected)
    #[inline]
    pub fn is_stack_overflow(&self, sp: usize) -> bool {
        if self.kernel_stack_bottom == 0 {
            return false;  // No stack allocated yet
        }
        sp < self.kernel_stack_bottom
    }

    /// Get pt_regs pointer at top of kernel stack
    ///
    /// pt_regs = (kernel_stack_top - sizeof(PtRegs))
    ///
    /// In Rux, kernel_stack stores the stack top address, so:
    /// pt_regs = kernel_stack - sizeof(PtRegs)
    ///
    /// # Returns
    /// Pointer to pt_regs structure at top of kernel stack, or null if no stack
    #[inline]
    pub fn pt_regs(&self) -> *mut super::super::arch::pt_regs::PtRegs {
        if let Some(stack_top) = self.kernel_stack {
            let pt_regs_addr = stack_top as usize - core::mem::size_of::<super::super::arch::pt_regs::PtRegs>();
            pt_regs_addr as *mut super::super::arch::pt_regs::PtRegs
        } else {
            core::ptr::null_mut()
        }
    }

    /// Typed view of the task's trap frame: the PtRegs-sized window at
    /// the top of its own kernel stack.
    ///
    /// Single audited edge for the fork/copy_thread family (round 3b):
    /// when `kernel_stack` is Some, the window `stack_top -
    /// size_of::<PtRegs>()` is this task's own trap frame (the stack is
    /// allocated with the task slot and the frame lives at its fixed
    /// top offset — the same layout contract the raw `pt_regs()`
    /// exposes); the returned reference is exclusive while the task is
    /// not yet enqueued (fork builds the child before `enqueue_task`
    /// transfers ownership).
    #[inline]
    pub fn pt_regs_mut(
        &mut self,
    ) -> Option<&'static mut super::super::arch::pt_regs::PtRegs> {
        if self.kernel_stack.is_some() {
            let p = self.pt_regs();
            // SAFETY: kernel_stack is Some, so pt_regs() is non-null and
            // points into this task's own kernel-stack window; the slot
            // discipline (boxed once, freed only via free_task_slot)
            // keeps the frame alive for the slot's 'static lifetime.
            Some(crate::process::r#unsafe::frame_mut(p))
        } else {
            None
        }
    }

    /// Shared read view of the task's trap frame (round-4 companion to
    /// `pt_regs_mut`): the PtRegs-sized window at the top of its own
    /// kernel stack, for diagnostic readers that must not mutate.
    ///
    /// Same layout contract as `pt_regs()`/`pt_regs_mut()` (the stack
    /// is allocated with the task slot and the frame lives at its
    /// fixed top offset); the reference is shared, so concurrent reads
    /// while another CPU runs the task see a frame mid-flight — the
    /// caller's diagnostic-read contract.
    #[inline]
    pub fn pt_regs_ref(&self) -> Option<&'static super::super::arch::pt_regs::PtRegs> {
        if self.kernel_stack.is_some() {
            let p = self.pt_regs();
            // SAFETY: kernel_stack is Some, so pt_regs() is non-null and
            // points into this task's own kernel-stack window (same
            // argument as pt_regs_mut); shared reads only.
            Some(crate::process::r#unsafe::frame_ref(p))
        } else {
            None
        }
    }

    /// Has address space (user process)
    #[inline]
    pub fn has_address_space(&self) -> bool {
        self.address_space.is_some()
    }

    /// Get OOM score adjustment
    #[inline]
    pub fn oom_score_adj(&self) -> i32 {
        self.oom_score_adj.load(Ordering::Acquire)
    }

    /// Set OOM score adjustment, clamped to [-1000, 1000]
    #[inline]
    pub fn set_oom_score_adj(&self, adj: i32) {
        self.oom_score_adj.store(adj.clamp(-1000, 1000), Ordering::Release);
    }

    /// Get the stored I/O scheduling priority
    /// ((class << 13) | level; 0 = NONE/0)
    #[inline]
    pub fn ioprio(&self) -> i32 {
        self.ioprio.load(Ordering::Acquire)
    }

    /// Set the stored I/O scheduling priority (ioprio_set).
    #[inline]
    pub fn set_ioprio(&self, v: i32) {
        self.ioprio.store(v, Ordering::Release);
    }

    /// Check if has file descriptor table
    #[inline]
    pub fn has_fdtable(&self) -> bool {
        self.fdtable.is_some()
    }

    /// Get file descriptor table (Option version)
    #[inline]
    pub fn try_fdtable(&self) -> Option<&FdTable> {
        self.fdtable.as_ref().map(|arc| arc.as_ref())
    }

    /// True when this task's fdtable Arc has no other owner (no
    /// CLONE_FILES sharers) — the gate for the reference's fdget
    /// no-refcount fast path (files_lookup_fd_raw + FDPUT): unshared
    /// means no other thread can close/dup concurrently, so a syscall
    /// may borrow the File straight from the slot.
    #[inline]
    pub fn fdtable_unshared(&self) -> bool {
        self.fdtable
            .as_ref()
            .map(|a| alloc::sync::Arc::strong_count(a) == 1)
            .unwrap_or(false)
    }

    /// task_ppid(me) shape: the leader's parent Task pointer behind the
    /// same plausibility screen ppid() uses (a non-leader thread's
    /// parent field can be an unlinked snapshot naming freed memory).
    #[inline]
    pub fn parent_task_ptr(&self) -> *const Task {
        match self.parent {
            Some(parent_ptr) => {
                if !Self::is_plausible_task_ptr(parent_ptr) {
                    return core::ptr::null();
                }
                parent_ptr
            }
            None => core::ptr::null(),
        }
    }

    /// Get file descriptor table
    #[inline]
    pub fn fdtable(&self) -> &FdTable {
        self.fdtable.as_ref().expect("FdTable not initialized")
    }

    /// Get mutable reference to file descriptor table (Option version)
    /// Note: FdTable has interior mutability, &self is sufficient
    #[inline]
    pub fn try_fdtable_mut(&self) -> Option<&FdTable> {
        self.fdtable.as_ref().map(|arc| arc.as_ref())
    }

    /// Get mutable reference to file descriptor table
    /// Note: FdTable has interior mutability, so &FdTable is sufficient for mutation
    #[inline]
    pub fn fdtable_mut(&self) -> &FdTable {
        self.fdtable.as_ref().expect("FdTable not initialized")
    }

    /// Set file descriptor table
    #[inline]
    pub fn set_fdtable(&mut self, fdtable: Option<alloc::sync::Arc<FdTable>>) {
        self.fdtable = fdtable;
    }

    /// Get Arc reference to fdtable (for CLONE_FILES)
    pub fn fdtable_arc(&self) -> Option<alloc::sync::Arc<FdTable>> {
        self.fdtable.clone()
    }

    /// Get Arc reference to signal struct (for CLONE_SIGHAND)
    pub fn signal_arc(&self) -> Option<alloc::sync::Arc<SignalStruct>> {
        self.signal.clone()
    }

    /// Set signal struct
    pub fn set_signal(&mut self, signal: Option<alloc::sync::Arc<SignalStruct>>) {
        self.signal = signal;
    }

    /// Set parent process
    pub fn set_parent(&mut self, parent: *const Task) {
        if parent.is_null() {
            self.parent = None;
        } else {
            self.parent = Some(parent);
        }
    }

    /// Get parent process pointer
    #[inline]
    pub fn parent_ptr(&self) -> Option<*const Task> {
        self.parent
    }

    /// Get exit code
    #[inline]
    /// Signal the parent receives at this task's exit (clone exit_signal).
    pub fn get_exit_signal(&self) -> u8 {
        self.exit_signal
    }

    /// Set the exit signal (clone/clone3; only meaningful before the
    /// task first runs).
    pub fn set_exit_signal(&mut self, sig: u8) {
        self.exit_signal = sig;
    }

    pub fn exit_code(&self) -> i32 {
        self.exit_code
    }

    /// Set exit code
    #[inline]
    pub fn set_exit_code(&mut self, code: i32) {
        self.exit_code = code;
    }

    /// Get stop signal
    #[inline]
    pub fn stop_signal(&self) -> i32 {
        self.stop_signal
    }

    /// Set stop signal
    #[inline]
    pub fn set_stop_signal(&mut self, sig: i32) {
        self.stop_signal = sig;
    }

    // ==================== ptrace / coredump accessors (P1) ====================

    /// PID of the tracing process (0 = not traced).
    #[inline]
    pub fn tracer_pid(&self) -> u32 {
        self.ext.tracer_pid.load(Ordering::Acquire)
    }

    /// Set the tracing process PID.
    #[inline]
    pub fn set_tracer_pid(&self, pid: u32) {
        self.ext.tracer_pid.store(pid, Ordering::Release);
    }

    /// Set PTRACE_SETOPTIONS word.
    #[inline]
    pub fn set_ptrace_options(&self, opts: u64) {
        self.ext.ptrace_options.store(opts, Ordering::Release);
    }

    /// Mark a signal as re-injected by PTRACE_CONT (deliver without
    /// another trace stop).
    #[inline]
    pub fn set_ptrace_sigdeliver(&self, sig: u32) {
        self.ext.ptrace_sigdeliver.store(sig, Ordering::Release);
    }

    /// Consume the re-inject mark; returns the signal number (0 = none).
    #[inline]
    pub fn take_ptrace_sigdeliver(&self) -> u32 {
        self.ext.ptrace_sigdeliver.swap(0, Ordering::AcqRel)
    }

    /// Peek the re-inject mark without consuming it.
    #[inline]
    pub fn ptrace_sigdeliver_peek(&self) -> u32 {
        self.ext.ptrace_sigdeliver.load(Ordering::Acquire)
    }

    /// Stash the siginfo of the signal that caused the current trace stop.
    pub fn set_ptrace_siginfo(&self, info: crate::signal::SigInfo) {
        *self.ext.ptrace_siginfo.lock() = Some(info);
    }

    /// Read the stashed trace-stop siginfo (PTRACE_GETSIGINFO).
    pub fn ptrace_siginfo(&self) -> Option<crate::signal::SigInfo> {
        *self.ext.ptrace_siginfo.lock()
    }

    /// Clear the stashed trace-stop siginfo (CONT/DETACH).
    pub fn clear_ptrace_siginfo(&self) {
        *self.ext.ptrace_siginfo.lock() = None;
    }

    /// Arm a software single step: the instruction at `addr` is displaced
    /// by EBREAK; `insn` is the original word to restore on the trap.
    #[inline]
    pub fn arm_single_step(&self, addr: u64, insn: u64) {
        self.ext.ss_addr.store(addr, Ordering::Release);
        self.ext.ss_saved_insn.store(insn, Ordering::Release);
        self.ext.ss_active.store(true, Ordering::Release);
    }

    /// Disarm the software single step (restore already happened).
    #[inline]
    pub fn clear_single_step(&self) {
        self.ext.ss_active.store(false, Ordering::Release);
    }

    /// Is a software single step pending?
    #[inline]
    pub fn single_step_active(&self) -> bool {
        self.ext.ss_active.load(Ordering::Acquire)
    }

    /// Address of the displaced (EBREAK) instruction.
    #[inline]
    pub fn single_step_addr(&self) -> u64 {
        self.ext.ss_addr.load(Ordering::Acquire)
    }

    /// Original word of the displaced instruction.
    #[inline]
    pub fn single_step_saved_insn(&self) -> u64 {
        self.ext.ss_saved_insn.load(Ordering::Acquire)
    }

    /// Accessor for the stop-reported flag (ptrace stop notification).
    #[inline]
    pub fn stop_reported(&self) -> &core::sync::atomic::AtomicBool {
        &self.stop_reported
    }

    /// Mark that a core file was written for this task's death.
    #[inline]
    pub fn set_core_dumped(&self) {
        self.ext.core_dumped.store(true, Ordering::Release);
    }

    /// Was a core file written for this task's death? (WCOREDUMP bit)
    #[inline]
    pub fn core_dumped(&self) -> bool {
        self.ext.core_dumped.load(Ordering::Acquire)
    }

    /// Get process name (comm)
    #[inline]
    pub fn comm(&self) -> &[u8; 16] {
        &self.comm
    }

    /// Print-safe comm as a str: NUL-terminated slice, lossy-decoded.
    ///
    /// A comm holds ARBITRARY bytes (prctl PR_SET_NAME copies raw, and a
    /// >15-byte UTF-8 thread name truncates MID-CHARACTER), so the old
    /// `from_utf8_unchecked(comm).trim_end_matches('\0')` sites handed
    /// char-aware formatting (Debug escaping, truncation) an invalid &str
    /// and panicked the kernel ("start byte index N is not a char
    /// boundary", lb120 @672s in a KOT-KILL vnames Debug print).
    /// Zero-copy when the bytes are valid UTF-8.
    pub fn comm_str(&self) -> alloc::borrow::Cow<'_, str> {
        let raw: &[u8] = self.comm();
        let len = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        alloc::string::String::from_utf8_lossy(&raw[..len])
    }

    /// Set process name (comm), max 15 chars + NUL
    #[inline]
    pub fn set_comm(&mut self, name: &[u8]) {
        let len = name.len().min(15);
        self.comm[..len].copy_from_slice(&name[..len]);
        self.comm[len] = 0;
    }

    /// Get process group ID
    #[inline]
    pub fn pgid(&self) -> u32 {
        self.pgid
    }

    /// Set process group ID
    #[inline]
    pub fn set_pgid(&mut self, pgid: u32) {
        self.pgid = pgid;
    }

    /// Get session ID
    #[inline]
    pub fn sid(&self) -> u32 {
        self.sid
    }

    /// Set session ID
    #[inline]
    pub fn set_sid(&mut self, sid: u32) {
        self.sid = sid;
    }

    /// Has this task exec'd at least once? (setpgid EACCES rule)
    #[inline]
    pub fn has_execed(&self) -> bool {
        self.execed.load(core::sync::atomic::Ordering::Acquire)
    }

    /// Mark this task as having exec'd (first successful execve).
    #[inline]
    pub fn mark_execed(&self) {
        self.execed.store(true, core::sync::atomic::Ordering::Release);
    }

    // ==================== Process Tree Management ====================

    /// Recover the Task that owns a `sibling` list node (container_of).
    ///
    /// Pure pointer arithmetic — no dereference — so the result is only
    /// as valid as the node it came from: callers keep their existing
    /// PROCESS_TREE_LOCK / pin / liveness contracts for any use.
    #[inline]
    pub fn from_sibling_node(node: *mut ListHead) -> *mut Task {
        (node as usize - offset_of!(Task, sibling)) as *mut Task
    }

    /// Get first child
    ///
    ///
    /// # Returns
    /// Some(child pointer) if has children, None otherwise
    pub fn first_child(&self) -> Option<*mut Task> {
        // children list may be empty
        if self.children.is_empty() {
            return None;
        }
        // Get first sibling node from children list head, then recover
        // the Task containing it (container_of; no dereference here).
        Some(Task::from_sibling_node(self.children.next))
    }

    /// Get next sibling
    ///
    /// Contract: caller must hold PROCESS_TREE_LOCK (or guarantee no
    /// concurrent list modification) before dereferencing the result.
    ///
    /// # Returns
    /// Some(pointer) if has next sibling, None otherwise
    pub fn next_sibling(&self) -> Option<*mut Task> {
        // If parent's children list head not saved, not in any parent's children list
        if self.parent_children_head.is_null() {
            return None;
        }

        let next_sibling = self.sibling.next;

        // If next points to parent's children list head, reached list end
        if next_sibling == self.parent_children_head {
            return None;
        }

        // Task struct pointer containing that sibling (no dereference)
        Some(Task::from_sibling_node(next_sibling))
    }

    /// Check if has children
    ///
    /// # Returns
    /// true if has children, false otherwise
    pub fn has_children(&self) -> bool {
        !self.children.is_empty()
    }

    /// Add child to process tree
    ///
    /// Contract: `self` is a valid parent, `child` is a valid,
    /// fully-constructed task not yet in any process tree.
    ///
    /// # Arguments
    /// - `child`: Child process pointer to add
    pub fn add_child(&self, child: *mut Task) {
        let _lock = PROCESS_TREE_LOCK.lock();
        // Caller contract above — child is a valid task pointer;
        // PROCESS_TREE_LOCK is held for the whole link operation.
        let c = crate::process::r#unsafe::borrow_task_mut(child);

        // R9 tripwire: linking an ALREADY-LINKED child double-links it —
        // one unlink leaves the other parent's list dangling, and the
        // freed/reused (zeroed) node shows up as the NULL children-walk
        // fault. Photograph the culprit before it strikes.
        if let Some(old) = c.parent {
            if old as *const Task != self as *const Task {
                let msg = b"TREE: DOUBLE add_child old=0x";
                for &b in msg { crate::console::putchar_no_lock(b); }
                let mut sh = 64;
                while sh > 0 {
                    sh -= 4;
                    let nb = ((old as usize >> sh) & 0xF) as u8;
                    crate::console::putchar_no_lock(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
                }
                for &b in b" new=0x" { crate::console::putchar_no_lock(b); }
                sh = 64;
                while sh > 0 {
                    sh -= 4;
                    let nb = ((self as *const Task as usize >> sh) & 0xF) as u8;
                    crate::console::putchar_no_lock(if nb < 10 { b'0' + nb } else { b'a' + nb - 10 });
                }
                crate::console::putchar_no_lock(b'\n');
            }
        }

        // Set child's parent
        c.parent = Some(self as *const _ as *mut Task);

        // Save parent's children list head pointer (for next_sibling boundary check)
        c.parent_children_head = &self.children as *const _ as *mut ListHead;

        // Link child's sibling to parent's children list
        // (add_tail; tree lock held — INV-PROC-LIST-1).
        crate::process::r#unsafe::list_add_tail(
            &mut c.sibling,
            &self.children as *const _ as *mut ListHead,
        );
    }

    /// Remove child from process tree
    ///
    /// Contract: `child` is a valid task pointer currently linked in
    /// this parent's children list.
    ///
    /// # Arguments
    /// - `child`: Child process pointer to remove
    pub fn remove_child(&self, child: *mut Task) {
        let _lock = PROCESS_TREE_LOCK.lock();
        // Caller contract above — child is valid and linked here;
        // PROCESS_TREE_LOCK is held for the whole unlink.
        let c = crate::process::r#unsafe::borrow_task_mut(child);

        // Remove child's sibling from parent's children list
        // (the node is linked per contract; tree lock held).
        crate::process::r#unsafe::list_del(&mut c.sibling);

        // Reinitialize sibling list (prevent dangling pointer)
        c.sibling.init();

        // Clear parent pointer
        c.parent = None;

        // Clear parent children list head pointer
        c.parent_children_head = ptr::null_mut();
    }

    /// E8-REPAR: collect `self`'s children into `out`, PINNING each one
    /// (task_refcnt) inside the same PROCESS_TREE_LOCK critical section
    /// that walks the list.
    ///
    /// The reparent path used for_each_child, which returns bare
    /// pointers the instant the tree lock drops — while the reaper of
    /// those children (a sibling thread of the dying task's group, in
    /// its own wait4 via for_each_group_child) could concurrently
    /// release_task() them: unlink, free_pid, task_put, page back to
    /// the heap. The reparent loop then wrote through the freed node's
    /// sibling pointers (ListHead::del's `(*next).prev = prev` —
    /// arbitrary kernel-memory writes) and linked the freed node into
    /// the new parent's children list, leaving every later walk reading
    /// garbage task pointers. A pin taken under the same lock hold makes
    /// the free impossible until the mover task_put()s: release_task's
    /// final task_put finds refcnt > 1 and defers free_task_slot.
    ///
    /// Contract: `self` is a valid task; callers must Task::task_put
    /// every returned pointer exactly once.
    pub fn collect_children_pinned(&self, out: &mut alloc::vec::Vec<*mut Task>) {
        let _lock = PROCESS_TREE_LOCK.lock();
        let head = &self.children as *const _ as *mut ListHead;
        // head is self's live children list; the walk holds
        // PROCESS_TREE_LOCK, so every node stays linked for the walk
        // (INV-PROC-LIST-1).
        crate::process::r#unsafe::list_for_each(head, |node| {
            let task_ptr = Task::from_sibling_node(node);
            // task_ptr owns this sibling node, which the tree lock keeps
            // linked — the task page cannot be freed mid-walk
            // (INV-PROC-BORROW-1).
            crate::process::r#unsafe::borrow_task(task_ptr)
                .task_refcnt
                .fetch_add(1, core::sync::atomic::Ordering::AcqRel);
            out.push(task_ptr);
        });
    }

    /// E8-REPAR: atomically move `child` from `self`'s children list to
    /// `dest`'s — validate + unlink + relink in ONE PROCESS_TREE_LOCK
    /// critical section.
    ///
    /// The old reparent code ran remove_child() and add_child() as two
    /// separately-locked sections with signal sends in between: a
    /// concurrent reaper unlinking the child in the gap left the second
    /// half writing into freed memory, and a `dest` chosen earlier (a
    /// surviving thread-group member) could itself complete do_exit and
    /// be swept before add_child — planting a permanently dangling
    /// parent pointer into the moved child (the exact ancestor the
    /// final8 subreaper walk panicked on). Validation (child still ours:
    /// parent AND parent_children_head) makes the vanishing-child case a
    /// clean "skip"; callers must pin `dest` for the duration.
    ///
    /// Returns true when the move happened; false means the child was
    /// concurrently unlinked (reaped / already moved) — do not touch it
    /// further beyond the caller's own task_put.
    ///
    /// Contract: `self`, `child` and `dest` are valid Task pointers;
    /// `child` is pinned by the caller; `dest` is pinned by the caller.
    pub fn move_child(&self, child: *mut Task, dest: *mut Task) -> bool {
        let _lock = PROCESS_TREE_LOCK.lock();
        // Caller contract above — child and dest are valid, pinned
        // tasks; PROCESS_TREE_LOCK is held for the whole move.
        let c = crate::process::r#unsafe::borrow_task_mut(child);
        let d = crate::process::r#unsafe::borrow_task_mut(dest);
        let still_ours = c.parent == Some(self as *const Task)
            && c.parent_children_head == &self.children as *const _ as *mut ListHead;
        if !still_ours {
            return false;
        }
        // Unlink — the same mutations remove_child performs (the node
        // is linked here per the validation above; tree lock held).
        crate::process::r#unsafe::list_del(&mut c.sibling);
        c.sibling.init();
        // Relink — the same mutations add_child performs.
        c.parent = Some(dest as *const Task);
        c.parent_children_head = &d.children as *const _ as *mut ListHead;
        // c.sibling was just re-initialized; d's children head is live
        // (dest pinned); tree lock held.
        crate::process::r#unsafe::list_add_tail(
            &mut c.sibling,
            &d.children as *const _ as *mut ListHead,
        );
        // E8-REPAR root cause (leg-29A): the moved child's thread-group
        // MEMBERS carry parent SNAPSHOTS (thread_group_join copies the
        // leader's parent verbatim; nothing maintained them), so a
        // reparented group left every member naming the OLD parent —
        // memory that gets reaped and recycled while the members live.
        // The member's own exit then walks release_task's
        // parent_ptr()/remove_child through freed memory (the sc304
        // signal-Arc Oops family; reproduced historically by the retired
        // treehunt probe + test/subreap_churn.c as TREE-SNAP-STALE).
        // Refresh every
        // member's snapshot to the new dest under this same lock hold —
        // the ring walk is the same bounded pattern the reparent member
        // scan uses, and members can't join/leave concurrently (both
        // take PROCESS_TREE_LOCK).
        let mut m = c.next_thread_ptr();
        let mut guard = 0u32;
        while (m as *const Task) != (child as *const Task)
            && !m.is_null()
            && guard < 65536
        {
            // Ring members stay linked while PROCESS_TREE_LOCK is held
            // (join/leave both take it); the walk is bounded.
            {
                let m_ref = crate::process::r#unsafe::borrow_task_mut(m);
                if m_ref.group_leader == child {
                    m_ref.parent = Some(dest as *const Task);
                    m_ref.parent_children_head = ptr::null_mut();
                }
                m = m_ref.next_thread_ptr();
            }
            guard += 1;
        }
        true
    }

    /// E8-REPAR: claim a zombie child for reaping — unlink it from
    /// whichever parent's children list it is on right now.
    ///
    /// do_wait() selected the zombie under the tree lock but released it
    /// before calling release_task(); two group members woken by the
    /// same SIGCHLD (glib reaper + main thread) could BOTH select the
    /// same zombie and double-release it: the second release_task()
    /// then locks/unlinks/frees through the already-freed Task. Claiming
    /// (unlink + parent=None) inside the selection critical section
    /// makes the second scanner simply not find the child anymore; the
    /// first reap proceeds (release_task skips its own unlink when
    /// parent is already None).
    ///
    /// Returns true when this caller won the claim.
    ///
    /// Contract: CALLER MUST HOLD PROCESS_TREE_LOCK (invoked from inside
    /// for_each_group_child's critical section); `child` is a valid,
    /// pinned-or-otherwise-still-linked Task.
    pub fn claim_child_locked(child: *mut Task) -> bool {
        // Caller holds PROCESS_TREE_LOCK and vouches for `child`.
        let c = crate::process::r#unsafe::borrow_task_mut(child);
        if c.parent.is_none() {
            return false; // already claimed / never linked
        }
        // Unlink from the CURRENT list — the child's own sibling pointers
        // are the ground truth for which list the node is on, so del()
        // heals the owner's list without naming it (node is linked:
        // parent was Some; tree lock held).
        crate::process::r#unsafe::list_del(&mut c.sibling);
        c.sibling.init();
        c.parent = None;
        c.parent_children_head = ptr::null_mut();
        true
    }

    /// Iterate over all children
    ///
    ///
    /// # Arguments
    /// - `f`: Closure to call for each child
    ///
    /// Contract: `self` is a valid task; PROCESS_TREE_LOCK is acquired
    /// internally, so the tree cannot change during iteration.
    pub fn for_each_child<F>(&self, mut f: F)
    where
        F: FnMut(&Task),
    {
        let _lock = PROCESS_TREE_LOCK.lock();
        let head = &self.children as *const _ as *mut ListHead;
        // head is self's live children list; the tree lock is held, so
        // every node stays linked for the whole walk
        // (INV-PROC-LIST-1).
        crate::process::r#unsafe::list_for_each(head, |node| {
            let task_ptr = Task::from_sibling_node(node);
            // The node owns this Task and the tree lock keeps it linked
            // (its parent cannot release it mid-walk).
            f(crate::process::r#unsafe::borrow_task(task_ptr));
        });
    }

    /// Iterate the children of EVERY member of the caller's thread group.
    ///
    /// Linux do_wait(): any thread of a process can wait for a child
    /// forked by ANY member of its thread group (kernel/exit.c walks
    /// `while_each_thread(tsk)` unless __WNOTHREAD). glib's "gmain"
    /// worker thread reaps children the main thread spawned
    /// (GChildWatchSource / GSubprocess), so a per-thread children scan
    /// makes those waitpid()s fail with ECHILD before the child is
    /// reaped. Only process products (fork / clone without
    /// CLONE_THREAD) are ever linked, so each child sits on exactly one
    /// member's list and no child is visited twice.
    ///
    /// Contract: caller must ensure self is valid. PROCESS_TREE_LOCK is
    /// acquired internally; it serializes ring joins/leaves and
    /// children-list mutations, so every member we walk is still linked
    /// (members unhook from the ring before their task memory is
    /// released).
    pub fn for_each_group_child<F>(&self, mut f: F)
    where
        F: FnMut(*mut Task),
    {
        let _lock = PROCESS_TREE_LOCK.lock();
        let start = self as *const Task as *mut Task;
        let mut member = start;
        let mut guard = 0u32;
        loop {
            // Ring members stay linked under PROCESS_TREE_LOCK.
            let head = &crate::process::r#unsafe::borrow_task(member).children
                as *const _ as *mut ListHead;
            // head is that member's live children list; tree lock held
            // for the whole walk (INV-PROC-LIST-1).
            crate::process::r#unsafe::list_for_each(head, |node| {
                f(Task::from_sibling_node(node));
            });
            // Ring walk (next_thread_ptr heals a NULL to self — a
            // single-threaded task terminates on the first step; same
            // ring-liveness contract as above).
            member = crate::process::r#unsafe::borrow_task(member).next_thread_ptr();
            if member == start || member.is_null() {
                break;
            }
            guard += 1;
            if guard > 65536 {
                break;
            }
        }
    }

    /// Find child by PID
    ///
    /// # Arguments
    /// - `pid`: Process ID to find child by PID
    ///
    /// # Returns
    /// Some(child pointer) if found, None otherwise
    ///
    /// Contract: `self` is a valid task; PROCESS_TREE_LOCK is acquired
    /// internally. The returned pointer is valid only under the caller's
    /// own liveness discipline (pin or tree lock).
    pub fn find_child_by_pid(&self, pid: Pid) -> Option<*mut Task> {
        let _lock = PROCESS_TREE_LOCK.lock();
        let head = &self.children as *const _ as *mut ListHead;
        let mut result = None;
        // head is self's live children list; tree lock held
        // (INV-PROC-LIST-1).
        crate::process::r#unsafe::list_for_each(head, |node| {
            let task_ptr = Task::from_sibling_node(node);
            // node owns this Task; tree lock keeps it linked.
            if crate::process::r#unsafe::borrow_task(task_ptr).pid == pid {
                result = Some(task_ptr);
            }
        });
        result
    }

    /// Get child count
    ///
    /// # Returns
    /// Number of children
    pub fn count_children(&self) -> usize {
        let _lock = PROCESS_TREE_LOCK.lock();
        let head = &self.children as *const _ as *mut ListHead;
        let mut count = 0;
        // head is self's live children list; tree lock held.
        crate::process::r#unsafe::list_for_each(head, |_| {
            count += 1;
        });
        count
    }


    /// Get reference to pending signal queue
    #[inline]
    pub fn pending(&self) -> &crate::signal::SigPending {
        &self.pending
    }

    // ==================== musl libc support (set_tid_address, set_robust_list) ====================

    /// Set clear_child_tid address
    ///
    /// When process exits, kernel clears value at this address
    #[inline]
    pub fn set_clear_child_tid(&mut self, tidptr: *mut i32) {
        self.ext.clear_child_tid = tidptr;
    }

    /// Get clear_child_tid address
    #[inline]
    pub fn clear_child_tid(&self) -> *mut i32 {
        self.ext.clear_child_tid
    }

    /// Set robust list
    ///
    /// Used for robust mutex implementation
    #[inline]
    pub fn set_robust_list(&mut self, head: *const u8, len: usize) {
        self.ext.robust_list_head = head;
        self.ext.robust_list_len = len;
    }

    /// Get robust list head pointer
    #[inline]
    pub fn robust_list_head(&self) -> *const u8 {
        self.ext.robust_list_head
    }

    /// Get robust list length
    #[inline]
    pub fn robust_list_len(&self) -> usize {
        self.ext.robust_list_len
    }

    /// Get current brk value
    #[inline]
    pub fn get_brk(&self) -> u64 {
        self.brk.load(core::sync::atomic::Ordering::Acquire)
    }

    /// Set brk value
    #[inline]
    pub fn set_brk(&self, value: u64) {
        self.brk.store(value, core::sync::atomic::Ordering::Release);
    }

    /// Get current working directory
    pub fn get_cwd(&self) -> alloc::boxed::Box<[u8]> {
        if let Some(ref fs) = self.fs {
            fs.cwd_slice()
        } else {
            alloc::boxed::Box::from(&b"/"[..])
        }
    }

    /// Set current working directory
    pub fn set_cwd(&self, path: &[u8]) {
        if let Some(ref fs) = self.fs {
            fs.set_cwd(path);
        }
    }

    /// Get root directory (real-namespace normalized path; "/" when not
    /// chrooted). P1 chroot: path_lookup prefixes absolute paths with it.
    pub fn get_root(&self) -> alloc::boxed::Box<[u8]> {
        if let Some(ref fs) = self.fs {
            fs.get_root().into_boxed_slice()
        } else {
            alloc::boxed::Box::from(&b"/"[..])
        }
    }

    /// Set root directory (chroot). `path` must be a normalized
    /// real-namespace absolute path.
    pub fn set_root(&self, path: &[u8]) {
        if let Some(ref fs) = self.fs {
            fs.set_root(path);
        }
    }

    /// Get Arc reference to fs_struct (for CLONE_FS)
    pub fn fs_arc(&self) -> Option<alloc::sync::Arc<crate::fs::types::FsStruct>> {
        self.fs.clone()
    }

    /// (perf-path lane) Chroot probe WITHOUT cloning the fs Arc — the
    /// fs_arc() clone (an atomic refcount pair) ran once per path walk
    /// and several times per open's shortcut gates.
    pub fn process_rooted_cheap(&self) -> bool {
        match self.fs.as_ref() {
            Some(fs) => !fs.root_is_slash(),
            None => false,
        }
    }

    /// Set fs_struct
    pub fn set_fs(&mut self, fs: Option<alloc::sync::Arc<crate::fs::types::FsStruct>>) {
        self.fs = fs;
    }

    /// Get umask
    pub fn get_umask(&self) -> u32 {
        if let Some(ref fs) = self.fs {
            fs.get_umask()
        } else {
            0o022
        }
    }

    /// Set umask and return old value
    pub fn set_umask(&self, mask: u32) -> u32 {
        if let Some(ref fs) = self.fs {
            fs.set_umask(mask)
        } else {
            0o022
        }
    }

    // ==================== namespaces / seccomp (U1c) ====================

    /// Seccomp mode (0 disabled / 1 strict / 2 filter).
    pub fn seccomp_mode(&self) -> u32 {
        self.seccomp_mode.load(Ordering::Acquire)
    }

    /// Set seccomp mode.
    pub fn set_seccomp_mode(&self, mode: u32) {
        self.seccomp_mode.store(mode, Ordering::Release);
    }

    /// no_new_privs attribute (PR_SET_NO_NEW_PRIVS / PR_GET_NO_NEW_PRIVS).
    pub fn no_new_privs(&self) -> bool {
        self.no_new_privs.load(Ordering::Acquire)
    }

    pub fn set_no_new_privs(&self, v: bool) {
        self.no_new_privs.store(v, Ordering::Release);
    }

    /// Seccomp kill helper (signal.rs): force-queue SIGKILL to this task
    /// bypassing disposition/mask checks. Used by the STRICT mode gate and
    /// SECCOMP_RET_KILL — the default action of SIGKILL terminates the task
    /// on return to user mode.
    pub fn seccomp_kill(&self) {
        // Reference kernel semantics (kernel/seccomp.c do_seccomp /
        // seccomp_log + force_sig): a seccomp kill terminates the task
        // with SIGSYS (si_code SYS_SECCOMP) — NOT SIGKILL. LTP prctl04
        // pins the shape: the parent expects WTERMSIG == SIGSYS for both
        // STRICT and FILTER violations. force_sig also bypasses the
        // target's blocked mask and any caught handler: unblock SIGSYS
        // and reset its action to default before queueing, so the normal
        // delivery scan picks it up no matter what the target did.
        const SIGSYS: i32 = 31;
        // seccomp_kill is invoked on the CURRENT task from the syscall
        // gate (or on a fork-fresh child before it is shared); the
        // plain-field write goes through the same aliasing edge other
        // &self helpers use (the Task is never const —
        // INV-PROC-BORROW-1's current-task arm).
        crate::process::r#unsafe::borrow_task_mut(self as *const Task as *mut Task)
            .sigmask &= !(1u64 << (SIGSYS - 1));
        if let Some(sig_struct) = self.signal.as_ref() {
            let _ = sig_struct.set_action(SIGSYS, crate::signal::SigAction::new());
        }
        self.pending.add(SIGSYS);
        self.set_ti_flag(TIF_SIGPENDING);
        // signal_wake_up takes a valid task pointer; this method is
        // called with &self of a live task.
        crate::signal::signal_wake_up(self as *const Task as *mut Task);
    }

    /// This task's pid inside its own pid namespace (gettid view).
    pub fn ns_pid_local(&self) -> u32 {
        self.ns_local_pid
    }

    /// cgroup v2: membership pointer (NULL = implicit root, no limits).
    #[inline]
    pub fn cgroup_ptr(&self) -> *mut crate::sched::types::CgroupNode {
        self.cgroup_ptr
    }

    /// cgroup v2: set membership pointer (fork inherit / cgroup.procs
    /// migration). The pointee must come from a never-freed
    /// `Arc<CgroupNode>` allocation.
    #[inline]
    pub fn set_cgroup_ptr(&mut self, ptr: *mut crate::sched::types::CgroupNode) {
        self.cgroup_ptr = ptr;
    }

    /// cgroup v2 memory controller: bytes currently charged on behalf of
    /// this task (see field docs).
    #[inline]
    pub fn cgroup_mem_charged(&self) -> u64 {
        self.cgroup_mem_charged.load(Ordering::Acquire)
    }

    /// cgroup v2 memory controller: add to the task's charge ledger.
    #[inline]
    pub fn add_cgroup_mem_charged(&self, bytes: u64) {
        self.cgroup_mem_charged.fetch_add(bytes, Ordering::AcqRel);
    }

    /// cgroup v2 memory controller: subtract from the task's charge
    /// ledger, saturating at zero.
    #[inline]
    pub fn sub_cgroup_mem_charged(&self, bytes: u64) {
        let mut current = self.cgroup_mem_charged.load(Ordering::Acquire);
        loop {
            let next = current.saturating_sub(bytes);
            match self.cgroup_mem_charged.compare_exchange(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return,
                Err(actual) => current = actual,
            }
        }
    }

    /// Get executable file path
    pub fn get_exe_path(&self) -> &[u8] {
        &self.exe_path
    }

    /// Set user stack pointer
    pub fn set_user_sp(&self, sp: u64) {
        self.ti_user_sp.store(sp, core::sync::atomic::Ordering::Release);
    }

    /// Set executable file path
    pub fn set_exe_path(&mut self, path: &[u8]) {
        self.exe_path = Box::from(path);
    }
}

///
/// Optional: 100, 250, 300, 1000
const HZ: u32 = 100;

// ==================== Offset constants (for assembly use) ====================

/// Task struct thread_info field offsets
#[allow(dead_code)]
pub mod task_offsets {
    use super::*;

    pub const TI_FLAGS: usize = core::mem::offset_of!(Task, ti_flags);
    pub const TI_PREEMPT_COUNT: usize = core::mem::offset_of!(Task, ti_preempt_count);
    pub const TI_KERNEL_SP: usize = core::mem::offset_of!(Task, ti_kernel_sp);
    pub const TI_USER_SP: usize = core::mem::offset_of!(Task, ti_user_sp);
    pub const TI_CPU: usize = core::mem::offset_of!(Task, ti_cpu);
    pub const TI_A0: usize = core::mem::offset_of!(Task, ti_a0);
    pub const TI_A1: usize = core::mem::offset_of!(Task, ti_a1);
    pub const TI_A2: usize = core::mem::offset_of!(Task, ti_a2);
    /// R56-heal: __switch_to's affirm gate CASes this word (PICKED → LIVE).
    pub const TI_PICK_STATE: usize = core::mem::offset_of!(Task, ti_pick_state);
    /// R57-scribble: the thread_info neighborhood map — the pk= garbage
    /// captures prove a foreign store lands near ti_pick_state; these
    /// offsets let the dump identify which 8-byte field's halves overlap
    /// the pick-state word.
    pub const TI_CPUS_ALLOWED: usize = core::mem::offset_of!(Task, cpus_allowed);
    pub const TI_ON_CPU: usize = core::mem::offset_of!(Task, ti_on_cpu);
    pub const TI_RUNNING_ON_CPU: usize = core::mem::offset_of!(Task, running_on_cpu);
    pub const TI_HUNG_TASK_SINCE: usize = core::mem::offset_of!(Task, hung_task_since);
    pub const TI_TASK_REFCNT: usize = core::mem::offset_of!(Task, task_refcnt);
    pub const TI_JOURNAL_HANDLE: usize = core::mem::offset_of!(Task, journal_handle);

    // Other common field offsets
    pub const TASK_STATE: usize = core::mem::offset_of!(Task, state);
    pub const TASK_PID: usize = core::mem::offset_of!(Task, pid);
    pub const TASK_THREAD: usize = core::mem::offset_of!(Task, thread);
    pub const TASK_KERNEL_STACK_BOTTOM: usize = core::mem::offset_of!(Task, kernel_stack_bottom);
    // R20: KERNPANIC children-walk previously hardcoded 0x48/0x4c/0x758/0x768 —
    // all four drifted by 8 when journal_handle/ti_on_cpu/task_refcnt were
    // inserted before `state`. Derive them here so the walk can never go
    // stale again.
    pub const TASK_CHILDREN: usize = core::mem::offset_of!(Task, children);
    pub const TASK_SIBLING: usize = core::mem::offset_of!(Task, sibling);
    // KP-RING diag (x86 trap printer): raw offsets of the thread-group
    // ring fields (private to Task; offset_of! inside the module is legal).
    pub const TASK_GROUP_LEADER: usize = core::mem::offset_of!(Task, group_leader);
    pub const TASK_NEXT_THREAD: usize = core::mem::offset_of!(Task, next_thread);
    pub const TASK_NR_THREADS: usize = core::mem::offset_of!(Task, nr_threads);

    // oh-crash 98th (SPINLOCK-STUCK holder hunt): the DEADLOCK face's
    // wedge locks sit at page-aligned bases + 0x6dc/0xadc/0xedc inside
    // direct-buddy allocations — Task-shaped. These offsets let a serial
    // log's lock address be matched to its Task field OFFLINE (no boot
    // needed). Keep the raw static in .data (#[used]) — read it from the
    // ELF with nm + objdump. Offsets are of the FIELD; the Spinlock word
    // inside Spinlock<T> is at +0 of the field (locked is first).
    #[used]
    pub static TASK_LOCK_OFFSETS: [usize; 8] = [
        core::mem::size_of::<Task>(),
        core::mem::offset_of!(Task, dead_threads),
        core::mem::offset_of!(Task, mm_slot_lock),
        core::mem::offset_of!(Task, pending),
        core::mem::offset_of!(Task, wait_chldexit),
        core::mem::offset_of!(Task, sem_undo),
        core::mem::offset_of!(Task, posix_timers),
        core::mem::offset_of!(Task, rlimits),
    ];
}

/// Export offset constants
pub use task_offsets::*;

/// Get current process's file descriptor table
///
/// Returns None if no current task is set or task has no fdtable.
pub fn get_current_fdtable() -> Option<&'static crate::fs::types::FdTable> {
    match crate::sched::hotpath::current() {
        Some(c) => c.try_fdtable(),
        None => None,
    }
}


// ════════════════ task_ext ════════════════════════════
// (was task_ext.rs)
// THE TASK SERVICE BLOCK (rung F1, seed-core-design §4): the cold
// field families that moved out of `Task` behind ONE bridge field
// (`Task::ext: TaskExtHandle`). First family landed: the statistics
// and hunt counters — the per-task forensic rings (LASTSYS,
// last_fails, mm_ops, last_calls/init_calls/birth_calls), the
// accounting scalars (cutime/cstime, start_jiffies, cmaxrss,
// exit_maxrss) and did_exec.
//
// Placement discipline (binding, verified per field before the
// move): a field lives here ONLY when no hot path — context switch,
// trap entry/exit accounting, scheduler pick/walk, page fault, VFS
// namei — reads or writes it directly. The rings' writers are
// syscall-dispatch arms gated behind `rux:sysforensic`/`LASTSYS`
// statics (quiet boots pay one atomic load, unchanged); mm_ops is
// written only on the six address-space syscalls and read by the
// panic-time fault printers; the accounting scalars are reaped at
// wait4/exit and read by procfs. utime/stime (tick accounting),
// nvcsw/nivcsw (the per-switch note_switch writes), min_flt/maj_flt
// (page-fault path), schedstat_* (the __schedule edges) and the
// pick/wake/wait counters (entities.rs) are DIRECT hot-path writes
// and therefore stayed in the core block.
//
// INV-F1 (the bridge lifecycle): `Task::ext` is allocated at task
// creation — `Task::new` constructs the `TaskExt` in the same breath
// as the struct literal, so every constructor route (`new_task_at`,
// `new_idle_at`, the init task's placement) yields a Task whose ext
// is non-null — and freed at slot release: `free_task_slot`'s
// `drop_in_place` drops the Box (the FTS-LEAK branch withholds the
// page and the ext with it, exactly as for every other Task field).
// The slot lifecycle therefore owns BOTH blocks: no path may free
// one without the other, and no path may dereference a Task's ext
// after its slot has been released. If the bridge ever shows up in
// a switch-path profile, the field that pulled it in moves back
// into the core block — the split narrows, never widens the hot
// surface.
//
// Layout note: `TaskExt` is `#[repr(C)]` so any future framework
// side that must reach a specific field without naming the type can
// do so under a fixed-offset contract (the journal_seam
// INV-JSEAM-1 precedent); today no framework reader needs one — all
// access is through the typed handle. The type is framework-defined
// for now; it flips into the process service crate together
// with the namespace family (the last family), whose ns.rs types
// move in the same step per the F1 design.

impl TaskExt {
    pub fn new() -> Self {
        TaskExt {
            last_syscall_nr: AtomicU32::new(0),
            last_syscall_a0: AtomicU64::new(0),
            last_syscall_a1: AtomicU64::new(0),
            last_fails: [const { AtomicU32::new(0) }; 8],
            last_fails_cur: AtomicUsize::new(0),
            last_fails_a0: [const { AtomicU64::new(0) }; 8],
            last_fails_a1: [const { AtomicU64::new(0) }; 8],
            mm_ops: [const { AtomicU64::new(0) }; 32],
            mm_ops_cur: AtomicUsize::new(0),
            did_exec: AtomicBool::new(false),
            last_calls: [const { AtomicU32::new(0) }; LAST_CALLS_LEN],
            last_calls_cur: AtomicUsize::new(0),
            init_calls: [const { AtomicU32::new(0) }; INIT_CALLS_LEN],
            init_a0: [const { AtomicU64::new(0) }; INIT_CALLS_LEN],
            init_calls_cur: AtomicUsize::new(0),
            birth_calls: [const { AtomicU32::new(0) }; BIRTH_CALLS_LEN],
            birth_a0: [const { AtomicU64::new(0) }; BIRTH_CALLS_LEN],
            birth_done: AtomicBool::new(false),
            cutime_ticks: AtomicU64::new(0),
            cstime_ticks: AtomicU64::new(0),
            // Same stamp semantics as the old in-Task field: the
            // creation moment of THIS task (fork children get their
            // own stamp, as before).
            start_jiffies: AtomicU64::new(crate::timer::get_jiffies()),
            cmaxrss_kb: AtomicU64::new(0),
            exit_maxrss_kb: AtomicU64::new(0),
            tracer_pid: AtomicU32::new(0),
            ptrace_options: AtomicU64::new(0),
            ptrace_sigdeliver: AtomicU32::new(0),
            ptrace_siginfo: crate::sync::types::Spinlock::new(None),
            ss_addr: AtomicU64::new(0),
            ss_active: AtomicBool::new(false),
            ss_saved_insn: AtomicU64::new(0),
            core_dumped: AtomicBool::new(false),
            clear_child_tid: core::ptr::null_mut(),
            robust_list_head: core::ptr::null(),
            robust_list_len: 0,
            futex_wait_enqueued: AtomicBool::new(false),
            sigmask_restore: 0,
            sigmask_restore_valid: false,
        }
    }
}

impl TaskExtHandle {
    #[inline]
    pub fn new() -> Self {
        TaskExtHandle(alloc::boxed::Box::new(TaskExt::new()))
    }
}

impl Deref for TaskExtHandle {
    type Target = TaskExt;
    #[inline]
    fn deref(&self) -> &TaskExt {
        &*self.0
    }
}

impl DerefMut for TaskExtHandle {
    #[inline]
    fn deref_mut(&mut self) -> &mut TaskExt {
        &mut *self.0
    }
}


// ════════════════ task_ref ════════════════════════════
// (was task_ref.rs)
// Typed task handles — the raw-`*mut Task` consolidation of the
// unsafe-slim round (2026-11-19).
//
// The kernel's task lifecycle runs on raw `*mut Task` pointers stored
// in the PID hash, runqueues and the tp register. Two disciplines are
// documented at the sources (`pid_hash::pid_hash_lookup`'s doc and
// `Task::task_put`'s R12-5 note):
//
// 1. **Syscall-context short read** — an unpinned lookup is valid for
//    the duration of a short field read because the reaper (the
//    task's parent) cannot run on THIS CPU while the reader executes.
//    `TaskBorrow` is that discipline as a type.
// 2. **Pinned handle** — `pid_hash_lookup_pinned` takes a
//    `task_refcnt` pin that the caller must drop with `task_put` (the
//    last put frees the slot). `TaskRef` owns the pin: `Drop` calls
//    `task_put`, so no exit path can leak it.
//
// Both handle types `Deref` to `&Task` — the one consolidated
// dereference with the contract written once, instead of one
// `unsafe { (*t).field() }` per call site. No raw pointer escapes
// except through the explicit `as_ptr` escape hatch (required by the
// queue/hash internals that still live on raw pointers).
//
// Handing out `&Task`/`&mut Task` from a raw pointer is the same
// operation the call sites performed inline; the aliasing
// discipline is unchanged (a task is mutated only by the CPU that
// currently owns it or under the locks the callers already hold —
// exactly what the per-call-site SAFETY comments said before).

impl TaskBorrow {
    /// Borrow the task with `pid`, if it is in the PID hash.
    pub fn lookup(pid: u32) -> Option<Self> {
        let p = super::hotpath::pid_hash_lookup(pid);
        NonNull::new(p).map(TaskBorrow)
    }

    /// Escape hatch for APIs still taking raw pointers (queue/hash
    /// internals). The borrow discipline above still applies to the
    /// caller's use of the returned pointer.
    /// (INV-TASKREF-1 hatch row: shared-borrow read/write, no store,
    /// no yield — see the safety-and-performance audit table.)
    /// (INV-TASKREF-1 hatch row: address-of under the live pin.)
    pub fn as_ptr(&self) -> *mut Task {
        self.0.as_ptr()
    }

    /// Adopt an already-validated raw task handle (round 4): the ONE
    /// audited edge for handles that predate this type (tree-ring
    /// handles, walk-callback pointers). The caller proves the same
    /// contract `lookup` documents: a short read/write in syscall
    /// context on this CPU, no yield, no store.
    /// (INV-TASKREF-1 hatch row: adopt-once under the caller's
    /// liveness proof.)
    ///
    /// # Safety
    /// `p` must point at a live task (the caller's borrow/pin/tree-lock
    /// keeps the slot alive for the borrow's duration) and must not be
    /// adopted twice into handles that outlive that proof.
    pub unsafe fn from_raw(p: *mut Task) -> Option<Self> {
        NonNull::new(p).map(TaskBorrow)
    }
}

impl Deref for TaskBorrow {
    type Target = Task;

    fn deref(&self) -> &Task {
        // SAFETY: syscall-context contract (type doc): the reaper
        // cannot run on this CPU while the borrower executes, so the
        // task page cannot be freed under this reference for the
        // duration of the short read.
        unsafe { self.0.as_ref() }
    }
}

impl DerefMut for TaskBorrow {
    fn deref_mut(&mut self) -> &mut Task {
        // SAFETY: same contract as Deref; mutation additionally relies
        // on the caller's existing exclusion (this CPU owns the task,
        // or the caller holds the lock that serializes the write) —
        // the same argument the pre-conversion call sites made.
        unsafe { self.0.as_mut() }
    }
}

impl TaskRef {
    /// Look up and pin the task with `pid`; `None` if not found.
    pub fn lookup(pid: u32) -> Option<Self> {
        let p = super::hotpath::pid_hash_lookup_pinned(pid);
        NonNull::new(p).map(TaskRef)
    }

    /// Adopt an already-taken pin (e.g. one taken inline before this
    /// type existed). The returned handle owns it. `None` for null.
    pub fn from_pinned(p: *mut Task) -> Option<Self> {
        NonNull::new(p).map(TaskRef)
    }

    /// Escape hatch: keep the pin, hand out the raw pointer. The
    /// caller becomes responsible for the eventual `task_put`.
    /// (INV-TASKREF-1 hatch row: pin transfer, exactly-one task_put.)
    pub fn into_raw(self) -> *mut Task {
        let p = self.0.as_ptr();
        core::mem::forget(self);
        p
    }

    pub fn as_ptr(&self) -> *mut Task {
        self.0.as_ptr()
    }
}

impl Deref for TaskRef {
    type Target = Task;

    fn deref(&self) -> &Task {
        // SAFETY: the pin defers `free_task_slot` (task_put) until
        // after this handle is dropped, so the task page stays alive
        // and mapped for the whole lifetime of the handle.
        unsafe { self.0.as_ref() }
    }
}

impl DerefMut for TaskRef {
    fn deref_mut(&mut self) -> &mut Task {
        // SAFETY: same liveness as Deref; mutation exclusion is the
        // caller's (the pin serializes freeing, not writing).
        unsafe { self.0.as_mut() }
    }
}

impl Drop for TaskRef {
    fn drop(&mut self) {
        // SAFETY: this handle owns exactly one pin (lookup or
        // from_pinned); task_put is the documented release.
        super::types::Task::task_put(self.0.as_ptr());
    }
}


// ════════════════ pid_hash ════════════════════════════
// (was pid_hash.rs)
// RCU-protected PID hash table for O(1) task lookup by PID.
//
// Read path (lookup) uses RCU — no locks, just preempt_disable/enable.
// Write path (insert/remove) uses per-bucket spinlock.
//
// Uses a singly-linked list per bucket (Task.pid_hash_next) so that
// removal only writes the removed node's next pointer, not a neighbour's,
// making concurrent RCU traversal safe on SMP.

/// Number of hash buckets.
const PID_HASH_BUCKETS: usize = 256;

/// Per-bucket spinlock (simple TAS).
static BUCKET_LOCK: [AtomicBool; PID_HASH_BUCKETS] = [const { AtomicBool::new(false) }; PID_HASH_BUCKETS];


/// Per-bucket chain head (first task pointer, or null).
static BUCKET_HEAD: [AtomicPtr<Task>; PID_HASH_BUCKETS] = [const { AtomicPtr::new(ptr::null_mut()) }; PID_HASH_BUCKETS];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[inline]
fn bucket_index(pid: u32) -> usize {
    (pid as usize) % PID_HASH_BUCKETS
}

/// 64th: the bucket lock is taken with IRQs DISABLED and the saved flag is
/// returned for the paired unlock (restore). sc207 found the freeze class
/// (a timer-softirq for_each_task spinning on the very bucket an
/// interrupted task-context walker held — self-deadlock, both vCPUs 100%,
/// serial cut mid-line); the deferred-print mitigation narrowed but did
/// not close the window: ANY holder interruptible while holding (insert,
/// remove, or a walker's callback) freezes the machine the same way once
/// an IRQ-context walk lands in the gap (ohw40: both CPUs in
/// pid_hash::lock_bucket from binder::sleep_snapshot, BUCKET_LOCK[27]
/// held by an interrupted walker, hard freeze at t=40s). With the hold
/// IRQ-atomic a holder can never be interrupted out of its section, so
/// every spinner waits a bounded callback duration instead of forever.
#[inline]
fn lock_bucket(idx: usize) -> bool {
    let flags = crate::arch::cpu::save_and_disable_irq();
    while BUCKET_LOCK[idx]
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        core::hint::spin_loop();
    }
    flags
}

/// Try-lock a bucket. Returns false if it is currently held.
///
/// For wedge-diagnostic paths (debug taskdump): the code that most needs the
/// snapshot runs precisely when some CPU is stuck — possibly while holding
/// a bucket lock (sys_kill broadcast / send_signal_to_pgid / oom_kill call
/// send_signal → wake_up → runqueue lock INSIDE the for_each callback).
/// Blocking there would wedge the diagnostic itself.
/// 64th: IRQ-atomic twin of lock_bucket for the diagnostic try-path —
/// returns the saved IRQ flag on acquire (Some) so the paired
/// unlock_bucket can restore it; None (with IRQs restored) on refusal.
#[inline]
fn try_lock_bucket(idx: usize) -> Option<bool> {
    let flags = crate::arch::cpu::save_and_disable_irq();
    if BUCKET_LOCK[idx]
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .is_ok()
    {
        Some(flags)
    } else {
        crate::arch::cpu::restore_irq(flags);
        None
    }
}

#[inline]
fn unlock_bucket(idx: usize, flags: bool) {
    BUCKET_LOCK[idx].store(false, Ordering::Release);
    crate::arch::cpu::restore_irq(flags);
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Initialize the PID hash table (call once during boot).
pub fn init() {
    // AtomicPtr is already initialized to null via const, nothing extra needed.
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Insert a task into the PID hash table.
///
/// Called from `alloc_task_slot()` / idle init after task construction.
pub fn pid_hash_insert(task: *mut Task) {
    // task is a valid pointer to a Task that was just constructed and
    /// not yet visible to other CPUs (called before enqueue_task).
    let pid = crate::process::r#unsafe::borrow_task(task).pid();
    let idx = bucket_index(pid);

    let flags = lock_bucket(idx);
    // Prepend to chain (LIFO — O(1) insert): bucket lock held, task
    // valid and not yet in any chain (INV-PROC-BORROW-1).
    {
        let t = crate::process::r#unsafe::borrow_task_mut(task);
        let old_head = BUCKET_HEAD[idx].load(Ordering::Relaxed);
        t.pid_hash_next = old_head;
        BUCKET_HEAD[idx].store(task, Ordering::Release);
    }
    unlock_bucket(idx, flags);
}

/// Remove a task from the PID hash table by PID.
///
/// Called from `release_task()`. After this returns, the task may still be
/// visible to RCU readers until a grace period elapses.
pub fn pid_hash_remove(pid: u32) {
    let idx = bucket_index(pid);

    let flags = lock_bucket(idx);
    // Bucket lock held; the chain traversal uses Release/Acquire
    // ordering so no concurrent modifications can occur. Only the
    // removed node's next pointer is written (INV-PROC-PIDHASH-1).
    {
        let mut prev: *mut *mut Task = &BUCKET_HEAD[idx] as *const AtomicPtr<Task> as *mut *mut Task;
        let mut curr = BUCKET_HEAD[idx].load(Ordering::Acquire);
        while !curr.is_null() {
            if crate::process::r#unsafe::borrow_task(curr).pid() == pid {
                // sc82 forensics: record the unlink (thread OR leader) in
                // the global exit ring — the rwlock-freeze dump replays a
                // frozen tgid's deaths. No behavior change.
                crate::api::sync::note_task_unlink(
                    pid,
                    crate::process::r#unsafe::borrow_task(curr).tgid() as u32,
                );
                // Unlink: write *prev = curr->next.  This only touches the
                // _previous_ node's next pointer (or the bucket head), not
                // the removed node itself — safe for concurrent RCU readers
                // who may already be at `curr`.
                crate::process::r#unsafe::pid_hash_unlink_store(
                    prev,
                    crate::process::r#unsafe::borrow_task(curr).pid_hash_next,
                );
                break;
            }
            prev = &mut crate::process::r#unsafe::borrow_task_mut(curr).pid_hash_next;
            curr = crate::process::r#unsafe::borrow_task(curr).pid_hash_next;
        }
    }
    unlock_bucket(idx, flags);
}

/// Look up a task by PID (RCU read-side).
///
/// Returns a raw pointer to the Task, or null if not found.
/// The caller must NOT free the returned task while in the RCU read-side
/// critical section.
/// R12-5: pinned lookup for cross-CPU wake/notify paths — the returned
/// pointer is guaranteed alive until `Task::task_put`. Ordinary
/// `pid_hash_lookup` stays unpinned: its callers are syscall-context
/// short reads where the reaper (the task's parent) cannot run on THIS
/// cpu mid-read, and retrofitting 56 call sites with puts would leak
/// slots on every missed one.
pub fn pid_hash_lookup_pinned(pid: u32) -> *mut Task {
    let t = pid_hash_lookup(pid);
    if !t.is_null() {
        crate::process::r#unsafe::borrow_task(t)
            .task_refcnt
            .fetch_add(1, Ordering::AcqRel);
    }
    t
}

pub fn pid_hash_lookup(pid: u32) -> *mut Task {
    hotpath::rcu_read_lock();

    let idx = bucket_index(pid);
    // We are in an RCU read-side critical section. The bucket head is
    // loaded with Acquire ordering; any task found remains valid until
    // after rcu_read_unlock() plus a grace period
    // (INV-PROC-BORROW-1's RCU arm).
    let result = {
        let mut curr = BUCKET_HEAD[idx].load(Ordering::Acquire);
        let mut found: *mut Task = ptr::null_mut();
        while !curr.is_null() {
            let t = crate::process::r#unsafe::borrow_task(curr);
            if t.pid() == pid {
                found = curr;
                break;
            }
            curr = t.pid_hash_next;
        }
        found
    };

    hotpath::rcu_read_unlock();

    result
}

/// Iterate all tasks in the PID hash table.
///
/// Takes per-bucket locks sequentially. Used by the OOM killer.
pub fn pid_hash_for_each_task<F>(mut f: F)
where
    F: FnMut(&Task),
{
    for i in 0..PID_HASH_BUCKETS {
        let flags = lock_bucket(i);
        // SAFETY: We hold the bucket lock so no concurrent insert/remove
        // can modify the chain; every chained node is a live, fully
        // constructed Task (insert happens after construction), so a
        // shared reference for the callback's duration is sound. The
        // callback must not yield the CPU (the bucket lock is
        /// irq-save spin) nor retain the reference past its return.
        {
            let mut curr = BUCKET_HEAD[i].load(Ordering::Acquire);
            while !curr.is_null() {
                let t = crate::process::r#unsafe::borrow_task(curr);
                f(t);
                curr = t.pid_hash_next;
            }
        }
        unlock_bucket(i, flags);
    }
}

/// Non-blocking variant of `pid_hash_for_each_task` for wedge diagnostics.
///
/// Buckets whose lock is held are SKIPPED (the snapshot is partial), never
/// waited for. Returns the number of skipped buckets so the caller can
/// report that the dump is incomplete.
pub fn pid_hash_for_each_task_try<F>(mut f: F) -> usize
where
    F: FnMut(&Task),
{
    let mut skipped = 0;
    for i in 0..PID_HASH_BUCKETS {
        let Some(irq) = try_lock_bucket(i) else {
            skipped += 1;
            continue;
        };
        // SAFETY: We hold the bucket lock so no concurrent insert/remove can
        // modify the chain; tasks unlinked by a stalled holder are simply
        // missed by this snapshot; nodes are live, fully constructed
        // Tasks (same contract as pid_hash_for_each_task).
        {
            let mut curr = BUCKET_HEAD[i].load(Ordering::Acquire);
            while !curr.is_null() {
                let t = crate::process::r#unsafe::borrow_task(curr);
                f(t);
                curr = t.pid_hash_next;
            }
        }
        unlock_bucket(i, irq);
    }
    skipped
}

/// Collect EVERY task pid currently in the hash table.
///
/// History: this returned a fixed `[u32; 64]` snapshot — a
/// no-allocation shortcut that silently TRUNCATED the task list at 64
/// entries. /proc root readdir is built on it, so on task-dense
/// systems (the OH image: hundreds of live tasks by 14s) every pid
/// past the 64-slot window was invisible to userspace: `ps`, `ls
/// /proc` and every process-finder missed services wholesale (the mmi
/// syscall-diff probe polled /proc for 300s and never saw
/// `multimodalinput`, although the kernel's own watchdog walked the
/// same hash and found the task). The reference kernel has no such
/// cap — procfs lists the whole pid table. The walk now collects into
/// a Vec (the same shape as the /proc/[pid]/task branch: raw pushes
/// under the per-bucket locks, formatting after the walk).
pub fn pid_hash_collect_all() -> alloc::vec::Vec<u32> {
    let mut pids = alloc::vec::Vec::new();

    for i in 0..PID_HASH_BUCKETS {
        let flags = lock_bucket(i);
        // Bucket lock held so the chain cannot be modified concurrently.
        {
            let mut curr = BUCKET_HEAD[i].load(Ordering::Acquire);
            while !curr.is_null() {
                let t = crate::process::r#unsafe::borrow_task(curr);
                pids.push(t.pid());
                curr = t.pid_hash_next;
            }
        }
        unlock_bucket(i, flags);
    }

    pids
}


// ════════════════ wait ════════════════════════════════
// (was wait.rs)
// Wait Queue mechanism
//
// Core concepts:
// - Wait queues implement process blocking and waking
// - When a process needs to wait for a condition, it joins the wait queue and calls schedule()
// - When the condition is met, wake_up() wakes waiting processes

impl WaitQueueEntry {
    /// Create new wait queue entry
    ///
    /// # Arguments
    /// * `task` - Associated task
    /// * `exclusive` - Whether exclusive mode (mutual exclusion)
    pub fn new(task: *mut Task, exclusive: bool) -> Self {
        // SAFETY: every constructor passes the parking (current) task,
        // which is alive by construction; the pid read cannot fault
        // (kernel linear map) and cannot race the task's own free (a
        // task cannot free itself while executing its own park path).
        let pid = if task.is_null() {
            0
        } else {
            crate::process::r#unsafe::borrow_task(task).pid() as u32
        };
        Self {
            task,
            exclusive,
            woken: AtomicBool::new(false),
            pid,
        }
    }

    /// The registration-time pid (wake-drop fix R127 — see the field).
    pub fn registered_pid(&self) -> u32 {
        self.pid
    }

    /// Check if woken
    pub fn is_woken(&self) -> bool {
        self.woken.load(Ordering::Acquire)
    }

    /// Mark as woken
    pub fn set_woken(&self) {
        self.woken.store(true, Ordering::Release);
    }

    /// Clear the woken flag (lost-wake retry window).
    ///
    /// W1 (sticky-skip lost wake): `wake_up` marks the entry woken BEFORE
    /// calling `wake_up_process`, and refuses later wakes for an entry that
    /// is already flagged. When the underlying `wake_up_process` REFUSES the
    /// transition (target between its prepare_to_wait and its own
    /// `__schedule` link-drop — the R42 window — or a guard divergence),
    /// the waker's one-shot token is consumed while the task may still end
    /// up asleep: its `__schedule` reads the still-sleeping state, dequeues
    /// it, and no future wake retries this entry. Clearing the flag on a
    /// refused wake keeps the entry eligible so the NEXT event re-attempts
    /// the wake (idempotent: a genuinely running task is refused again
    /// harmlessly). The sc74 launcher IPC wedge — binder server threads
    /// parked >240 s owing replies with empty todos — has exactly this
    /// signature shape, so this closes the retry path; the stall-scan
    /// census in ipc/binder.rs still names any residual live occurrence.
    pub fn clear_woken(&self) {
        self.woken.store(false, Ordering::Release);
    }

    /// Get associated task
    pub fn task(&self) -> *mut Task {
        self.task
    }

    /// Check if exclusive mode
    pub fn is_exclusive(&self) -> bool {
        self.exclusive
    }
}

// Send/Sync impls live in unsafe.rs (wait.rs carries zero unsafe).

/// Total stale wait-queue entries evicted by the R127 wake-time screen
/// (wake-drop hunt diagnostic: a non-zero value on a boot means tasks
/// died with entries still parked — the exit-without-unwind leak class).
pub static WQ_STALE_EVICTED: AtomicU64 = AtomicU64::new(0);

/// Clamped decrement of a task's live-registration count (R127 helper).
/// The eviction path can name a Task page that was freed and RECYCLED —
/// its new incarnation owns the word now, and its count must never
/// underflow on the predecessor's behalf (a wrapped u32::MAX would pin
/// the exit-leak detector on that task forever). Clamps at zero.
///
/// `t` points into the kernel linear map (same read-contract as the
/// wake-time staleness screen; INV-PROC-BORROW-1's stale-screen arm —
/// the clamp keeps a recycled page's count honest either way).
fn wq_regs_dec(t: *mut Task, n: u32) {
    use core::sync::atomic::Ordering as AO;
    let t = crate::process::r#unsafe::borrow_task(t);
    let mut cur = t.wq_regs.load(AO::Relaxed);
    loop {
        let next = cur.saturating_sub(n);
        match t.wq_regs.compare_exchange_weak(
            cur,
            next,
            AO::Relaxed,
            AO::Relaxed,
        ) {
            Ok(_) => return,
            Err(v) => cur = v,
        }
    }
}

impl WaitQueueHead {
    /// Create new wait queue head
    pub const fn new() -> Self {
        Self {
            list: Spinlock::new(Vec::new()),
            waiters: AtomicUsize::new(0),
        }
    }

    /// Initialize wait queue head (runtime initialization)
    pub fn init(&self) {
        // Vec is automatically initialized
    }

    /// Add to wait queue
    ///
    /// # Arguments
    /// * `entry` - Wait queue entry
    pub fn add(&self, entry: WaitQueueEntry) {
        // Use lock_irqsave: wake_up is called from IRQ handlers
        let mut list = self.list.lock_irqsave();
        // R127 wake-drop hunt: per-task live-registration count (see
        // Task::wq_regs) — the exit-leak detector's datum.
        let t = entry.task();
        if !t.is_null() {
            // SAFETY: the entry's task is parking on this queue; the
            // count is a diagnostic mirror of the list we hold.
            crate::process::r#unsafe::borrow_task(t)
                .wq_regs
                .fetch_add(1, Ordering::Relaxed);
        }
        // Non-exclusive entries added to head, exclusive entries added to tail
        if entry.is_exclusive() {
            list.push(entry);
        } else {
            list.insert(0, entry);
        }
        // In-section (see `waiters`): published before the lock release.
        self.waiters.fetch_add(1, Ordering::Release);
    }

    /// Remove from wait queue
    ///
    /// # Arguments
    /// * `task` - Task to remove
    ///
    /// Returns how many entries were dropped (wake-drop hunt: a remove
    /// that drops MORE entries than the caller registered — or drops
    /// entries on a queue the caller never parked on — is the
    /// pointer-recycling theft signature; Task pages are recycled after
    /// free, so a stale entry keyed by the old pointer matches the new
    /// incarnation's remove).
    ///
    /// R127: entries are matched by (task pointer, registration pid) —
    /// a recycled Task page (same address, new pid) no longer matches,
    /// so a live task's unwait cannot strip a dead predecessor's stale
    /// entries. Those are evicted lazily by [`Self::wake_up`]'s
    /// staleness screen instead.
    pub fn remove_count(&self, task: *mut Task) -> usize {
        // SAFETY: every caller passes its own (live, running) task;
        // reading its pid is a const offset read in the linear map.
        let pid = if task.is_null() {
            0
        } else {
            crate::process::r#unsafe::borrow_task(task).pid() as u32
        };
        let mut list = self.list.lock_irqsave();
        let before = list.len();
        // R127 wake-drop hunt: mirror the per-task count for every entry
        // this remove actually drops (same (ptr, pid) match as the retain).
        let mut regs_dropped: u32 = 0;
        list.retain(|entry| {
            let hit = entry.task() == task && entry.registered_pid() == pid;
            if hit && !entry.task().is_null() {
                regs_dropped = regs_dropped.saturating_add(1);
            }
            !hit
        });
        let dropped = before - list.len();
        if regs_dropped > 0 {
            // SAFETY: task is the caller's own live task (see above).
            wq_regs_dec(task, regs_dropped);
        }
        if dropped > 0 {
            self.waiters.fetch_sub(dropped, Ordering::Release);
        }
        dropped
    }

    /// Remove from wait queue (drop count not needed).
    pub fn remove(&self, task: *mut Task) {
        let _ = self.remove_count(task);
    }

    /// Wake up processes in wait queue
    ///
    /// # Arguments
    /// * `mode` - Wake mode
    /// * `nr` - Number of processes to wake (0 means wake all)
    ///
    /// # Returns
    /// Actual number of processes woken
    ///
    /// LOCK ORDER (INV-LOCK-6, virtio-blk ABBA fix): this function takes the
    /// wait-queue lock and holds it across sched::wake_up_process for every
    /// woken waiter. Callers must therefore NOT hold any virtio driver lock
    /// (VIRTIO_PCI_BLK_LOCK, MMIO virtqueue lock, VIRTIO_*_PENDING) when
    /// calling wake_up/wake_up_all/add/prepare_to_wait/finish_wait: nesting
    /// a wait-queue lock inside a virtio lock is the BLK↔waitqueue ABBA that
    /// froze GNOME final6 with timer IRQs stopped. Drivers collect completed
    /// work under their locks and wake OUTSIDE them (timer.rs R12-3 pattern).
    pub fn wake_up(&self, mode: WakeUpHint, nr: usize) -> usize {
        // Lock-free empty fast path (leg-36): one Acquire load instead
        // of an irqsave lock/unlock pair for every wake of a queue
        // nobody sleeps on — the common case for polling consumers.
        // `waiters` is only moved inside the list lock's critical
        // sections, so 0 means the list is (or was, microseconds of
        // coherence ago, in which case the sleeper's condition
        // re-check owns the event) empty. See the field's doc block.
        if self.waiters.load(Ordering::Acquire) == 0 {
            return 0;
        }
        // Use lock_irqsave: this is called from interrupt handlers.
        let mut list = self.list.lock_irqsave();
        let mut awakened = 0;
        // Reference __wake_up_common budget discipline: ONLY exclusive
        // entries consume the nr_exclusive budget ("wake one" = wake ONE
        // EXCLUSIVE waiter); non-exclusive entries (poll/epoll
        // registrations) are woken on every pass WITHOUT consuming it —
        // so a data-arrival wake reaches every poller plus exactly one
        // exclusive reader. nr == 0 (wake_up_all) wakes EVERY entry,
        // exclusive or not (the old form stopped at the first exclusive
        // entry, which only worked because every registration was
        // non-exclusive; the AF_UNIX reader/writer queue split registers
        // exclusive waiters and needs the full reference semantics).
        let mut budget = if nr == 0 { usize::MAX } else { nr };

        // R12-1 (deferred-wake UAF — the definitive fix): wake WHILE STILL
        // HOLDING the queue lock. A not-yet-woken entry on the queue proves
        // its task has not passed finish_wait (which needs this same lock),
        // therefore has not returned to userspace, therefore has not
        // exited or been reaped — the entry's task pointer cannot go stale
        // under us. The old collect-then-drop-then-wake window let a
        // signal-woken sleeper run, exit and be FREED before our wake —
        // the wild-pointer enqueue and the S1 jump-to-user corruption.
        // Lock order queue->GRQ is safe: no GRQ-held path takes a
        // waitqueue lock (scheduler_tick polls the console queue BEFORE
        // acquiring the GRQ). The earlier PID-revalidation and reverted
        // RCU-wrap were interim measures.
        //
        // R127 AMENDMENT: the proof above covers tasks that unwind their
        // park through finish_wait/remove — but the OH wake-drop hunt
        // captured a poller thread that exited with its poll entries
        // still queued (no unwait ran; the entries then vanished only
        // via a recycled-page remove). Stale entries therefore EXIST;
        // the R127 screen below evicts them at wake time and the
        // (pointer, pid) key makes them unstealable meanwhile.
        //
        // R34: wake directly inside the iteration — the earlier
        // collect-into-a-Vec (heap allocation under this irqsave lock)
        // was pure overhead: waking here is equivalent (a woken task's
        // finish_wait on another CPU simply blocks on this lock, it
        // cannot mutate the list mid-iteration) and removes an in-lock
        // heap allocation from every event source's hot path.
        // Stale-entry eviction set (wake-drop fix R127): entries whose
        // task died without unwinding its park (observed on the OH input
        // hunt: a poller thread exited with its evdev poll entries still
        // queued — no finish_wait/remove ran, R12-1's liveness proof has
        // exactly this hole) keep naming a freed Task page. Once that
        // page is recycled the entry either strips ANOTHER task's
        // registrations (remove-by-pointer theft) or wakes the WRONG
        // (new) task. The registration-time pid (see WaitQueueEntry)
        // discriminates incarnations; mismatched entries are evicted
        // here, under the same lock, self-healing the queue at the next
        // wake. Fixed array, no heap: queues hold single-digit entries.
        let mut stale: [*mut Task; 16] = [core::ptr::null_mut(); 16];
        let mut stale_n: usize = 0;

        for entry in list.iter() {
            if budget == 0 {
                break;
            }

            // R127 staleness screen: one volatile pid read per entry
            // (the same offset wake_up_process reads anyway). A freed
            // Task carries TASK_POISON in pid; a recycled one carries
            // the NEW task's pid; both fail the compare.
            let entry_task = entry.task();
            let entry_stale = entry_task.is_null() || {
                // Read at a compile-time Task offset inside the kernel
                // linear map (same contract as the poison probe in
                // Task::is_plausible_task_ptr — cannot fault).
                let cur_pid = crate::process::r#unsafe::read_volatile_u32(
                    crate::process::r#unsafe::slot_ptr(
                        entry_task as *mut u8,
                        super::hotpath::task_offsets::TASK_PID,
                    ) as *const u32,
                );
                cur_pid != entry.registered_pid()
            };
            if entry_stale {
                if stale_n < stale.len() && !stale[..stale_n].contains(&entry_task) {
                    stale[stale_n] = entry_task;
                    stale_n += 1;
                }
                continue;
            }

            if !entry.is_woken() {
                entry.set_woken();

                let task = entry.task();
                if !task.is_null() {
                    // W1: a refused wake must not consume the entry's
                    // one-shot token — clear the flag so a later wake can
                    // retry (the task may still be heading into
                    // __schedule with a sleeping state).
                    // 66th: mode == Sync routes through the WF_SYNC wake
                    // (same transition, stacking dispatch — see
                    // Task::wake_up_impl).
                    let woke = if matches!(mode, WakeUpHint::Sync) {
                        crate::sched::hotpath::wake_up_process_sync(task)
                    } else {
                        crate::sched::hotpath::wake_up_process(task)
                    };
                    if !woke {
                        entry.clear_woken();
                        // Lost-park ring (rux:lostpark tracepoint; retired token rux:lostpark (retired debug=lostpark)): the refused-wake
                        // token return — pid, state bits, preempt_count.
                        if crate::debug::lostpark::armed() {
                            let st = crate::process::r#unsafe::borrow_task(task).state();
                            let pc = crate::process::r#unsafe::borrow_task(task).preempt_count();
                            let pid = crate::process::r#unsafe::borrow_task(task).pid();
                            crate::debug::lostpark::record(
                                4,
                                pid as u64,
                                st.bits() as u64,
                                pc as u32 as u64,
                                crate::timer::get_jiffies(),
                            );
                        }
                        // Do not count a refused wake as awakened: no task
                        // transition happened. Continue scanning so other
                        // entries on this queue still get their chance.
                        continue;
                    }
                }

                awakened += 1;

                if entry.is_exclusive() {
                    // Only an EXCLUSIVE entry consumes the budget (the
                    // reference's `if (ret && (flags & WQ_FLAG_EXCLUSIVE)
                    // && !--nr_exclusive) break;`).
                    budget -= 1;
                }
            } else {
                // sc100 (sticky-token lost wake): a woken-flagged entry
                // whose task is SLEEPING is a consumed one-shot token
                // whose wake never reached the task — the binder-stall
                // census captured the shape live (q=1 woken=1 while the
                // task sat INTERRUPTIBLE ∧ unlinked ∧ owned by no cpu
                // slot for minutes; every REKICK skipped the entry and
                // no other wake path exists). The token was spent, so
                // this queue will never deliver the task another wake
                // unless it is re-issued HERE. Re-issue it for sleeping
                // targets only: a genuinely running task's refusal is
                // benign (it either holds a live slot or the R46 heal
                // path owns it), and a freed target is caught by
                // wake_up_process's own poison/floor guards.
                let task = entry.task();
                if !task.is_null() {
                    // SAFETY: read-only state snapshot; wake_up_process's
                    // WP2/R15-6 guards handle stale task pointers.
                    let st = crate::process::r#unsafe::borrow_task(task).state();
                    if st.is_sleeping() || st.contains(super::types::TaskState::STOPPED) {
                        if crate::sched::hotpath::wake_up_process(task) {
                            awakened += 1;
                        }
                    }
                }
            }
        }

        // R127 eviction pass: drop the stale entries collected above
        // (still under the lock — no interleaving remove/add can race
        // it) and keep the `waiters` counter exact so the leg-36
        // fast path re-engages once the queue drains.
        if stale_n > 0 {
            let before = list.len();
            // Per-task count mirror: group the evicted entries' tasks so
            // each freed page's wq_regs decrements exactly once per
            // evicted entry naming it (the page may be recycled already —
            // the count then lands on the new incarnation, which the
            // reset-at-reuse list re-zeroes anyway; a DEAD-but-unfreed
            // page keeps an accurate count).
            let mut per_task: [(*mut Task, u32); 16] = [(core::ptr::null_mut(), 0); 16];
            let mut per_task_n = 0usize;
            list.retain(|e| {
                if stale[..stale_n].contains(&e.task()) {
                    let t = e.task();
                    if !t.is_null() {
                        if let Some(slot) =
                            per_task[..per_task_n].iter_mut().find(|(x, _)| *x == t)
                        {
                            slot.1 += 1;
                        } else if per_task_n < per_task.len() {
                            per_task[per_task_n] = (t, 1);
                            per_task_n += 1;
                        }
                    }
                    false
                } else {
                    true
                }
            });
            let evicted = before - list.len();
            for (t, n) in per_task[..per_task_n].iter() {
                // SAFETY: diagnostic count bookkeeping on a page that was
                // on this queue an instant ago (kernel linear map; the
                // same read-contract as the staleness screen above).
                wq_regs_dec(*t, *n);
            }
            if evicted > 0 {
                self.waiters.fetch_sub(evicted, Ordering::Release);
                WQ_STALE_EVICTED.fetch_add(evicted as u64, Ordering::Relaxed);
            }
        }
        drop(list);

        awakened
    }

    /// Wake all waiting processes (non-exclusive)
    pub fn wake_up_all(&self) -> usize {
        self.wake_up(WakeUpHint::Normal, 0)
    }

    /// 118th kick forensics: the queue's true state (waiters counter,
    /// per-entry task pid/state/woken/on_cpu) for the binder kick dump.
    /// Diagnostic only; takes the irqsave list lock briefly.
    pub fn debug_entries(&self) -> (usize, Vec<(u32, u32, bool, bool)>) {
        let list = self.list.lock_irqsave();
        let mut out = Vec::new();
        for e in list.iter() {
            let t = e.task();
            if t.is_null() {
                out.push((0, 0xFFFF_FFFF, e.is_woken(), false));
                continue;
            }
            // SAFETY: read-only diagnostic snapshot of a queued entry's
            // task; the entry keeps the task alive on this queue.
            let (pid, st, oncpu) = (
                crate::process::r#unsafe::borrow_task(t).pid(),
                crate::process::r#unsafe::borrow_task(t).state().bits(),
                crate::process::r#unsafe::borrow_task(t).on_cpu(),
            );
            out.push((pid as u32, st, e.is_woken(), oncpu));
        }
        (self.waiters.load(Ordering::Acquire), out)
    }

    /// Wake one process (exclusive)
    pub fn wake_up_one(&self) -> usize {
        self.wake_up(WakeUpHint::Normal, 1)
    }

    /// Wake the highest-priority SCHED_FIFO/RR entry (R6 rt-mutex
    /// handoff): scans the queue for un-woken RT-class entries and
    /// wakes the one with the highest rt_priority (FIFO among equals —
    /// list order). Returns None when NO RT-class entry is queued (or
    /// the wake was refused): the CALLER then runs its plain
    /// wake-one — no internal fallback, so exactly one handoff wake
    /// ever happens per release. Returns the woken task so the caller
    /// can route the priority-inheritance handoff to it.
    ///
    /// Same wake-under-lock contract as wake_up (R12-1): the woken
    /// entry cannot pass finish_wait while we hold the list lock, so
    /// its task pointer is live. Same R127 staleness screen.
    pub fn wake_up_one_hiprio(&self) -> Option<*mut Task> {
        if self.waiters.load(Ordering::Acquire) == 0 {
            return None;
        }
        let mut list = self.list.lock_irqsave();
        // Pick: RT-class entries only; highest rt_priority wins, list
        // order breaks ties (FIFO). One pass, no allocation.
        let mut best: Option<(usize, u32)> = None; // (index, rt_prio)
        for (i, entry) in list.iter().enumerate() {
            if entry.is_woken() {
                continue;
            }
            let t = entry.task();
            if t.is_null() {
                continue;
            }
            // Read at compile-time Task offsets inside the kernel
            // linear map (same contract as wake_up's screen).
            let cur_pid = crate::process::r#unsafe::read_volatile_u32(
                crate::process::r#unsafe::slot_ptr(
                    t as *mut u8,
                    super::hotpath::task_offsets::TASK_PID,
                ) as *const u32,
            );
            if cur_pid != entry.registered_pid() {
                continue; // R127 stale incarnation
            }
            let (is_rt, rt_prio) = {
                use super::types::SchedPolicy;
                let t_ref = crate::process::r#unsafe::borrow_task(t);
                let p = t_ref.policy();
                (
                    matches!(p, SchedPolicy::Fifo | SchedPolicy::Rr),
                    t_ref.rt_priority(),
                )
            };
            if !is_rt {
                continue;
            }
            if best.map_or(true, |(_, bp)| rt_prio > bp) {
                best = Some((i, rt_prio));
            }
        }
        let (i, _) = best?;
        let entry_task = list[i].task();
        list[i].set_woken();
        let woke = crate::sched::hotpath::wake_up_process(entry_task);
        if !woke {
            // W1 token discipline: a refused wake must not consume the
            // entry's one-shot flag.
            list[i].clear_woken();
            return None;
        }
        Some(entry_task)
    }

    /// Wake-drop probe snapshot (input:evdev_wake hunt): entries as
    /// (pid, state bits, woken flag, task ptr) so the push_event probe
    /// can key pre/post wake verdicts per entry AND read scheduler
    /// linkage for the exact task. Diagnostic only; irqsave list lock.
    pub fn debug_entries_ptr(&self) -> (usize, Vec<(u32, u32, bool, *mut Task)>) {
        let list = self.list.lock_irqsave();
        let mut out = Vec::new();
        for e in list.iter() {
            let t = e.task();
            if t.is_null() {
                out.push((0, 0xFFFF_FFFF, e.is_woken(), t));
                continue;
            }
            // SAFETY: read-only diagnostic snapshot; the entry keeps the
            // task alive on this queue while the lock is held.
            let (pid, st) = (
                crate::process::r#unsafe::borrow_task(t).pid(),
                crate::process::r#unsafe::borrow_task(t).state().bits(),
            );
            out.push((pid as u32, st, e.is_woken(), t));
        }
        (self.waiters.load(Ordering::Acquire), out)
    }

    /// Lock-free proof of waiters (the `waiters` fast-path counter):
    /// true when at least one entry is queued. Callers that only need
    /// "is anyone parked here?" (e.g. the idle loop's UART drain gate,
    /// which skips a port-read VM exit when no reader sleeps) use this
    /// instead of taking the irqsave list lock.
    pub fn has_sleepers(&self) -> bool {
        self.waiters.load(Ordering::Acquire) != 0
    }

    /// Diagnostic: entries currently queued, and how many carry the
    /// woken flag (VW forensic instrumentation — a woken-flagged entry
    /// whose task sleeps is the sticky-skip lost-wake signature).
    pub fn vw_queue_census(&self) -> (usize, usize) {
        let list = self.list.lock_irqsave();
        let total = list.len();
        let woken = list.iter().filter(|e| e.is_woken()).count();
        drop(list);
        (total, woken)
    }

    /// Try-lock variant for IRQ-context diagnostics: None when the list
    /// lock is held (never block — the holder may be the interrupted
    /// context on this very CPU).
    pub fn vw_queue_census_try(&self) -> Option<(usize, usize)> {
        let list = self.list.try_lock_irqsave()?;
        let total = list.len();
        let woken = list.iter().filter(|e| e.is_woken()).count();
        drop(list);
        Some((total, woken))
    }

    /// Prepare to wait: atomically add entry to queue AND set task state.
    ///
    /// This prevents the classic lost-wakeup race where a waker finds the
    /// entry on the queue but the task is still RUNNING (is_sleeping()=false),
    /// marks the entry as woken, then the task sets INTERRUPTIBLE and vanishes
    /// from the runqueue — never to be woken again.
    ///
    /// By holding the waitqueue lock across both operations, the waker (which
    /// also holds this lock) will always see a consistent state.
    pub fn prepare_to_wait(&self, task: *mut Task, exclusive: bool, interruptible: bool) {
        let entry = WaitQueueEntry::new(task, exclusive);
        let mut list = self.list.lock_irqsave();

        // Set task state BEFORE adding to queue (both under same lock).
        // Waker holds this same lock, so it will see either:
        //   - task still RUNNING, entry not on queue → no action (correct)
        //   - task INTERRUPTIBLE, entry on queue → wake_up succeeds (correct)
        if interruptible {
            crate::process::r#unsafe::borrow_task(task).set_state(
                super::types::TaskState::new(
                    super::types::TaskState::INTERRUPTIBLE));
        } else {
            crate::process::r#unsafe::borrow_task(task).set_state(
                super::types::TaskState::new(
                    super::types::TaskState::UNINTERRUPTIBLE));
        }

        // Add entry to queue
        if exclusive {
            list.push(entry);
        } else {
            list.insert(0, entry);
        }
        // In-section (see `waiters`): the increment must be published
        // before the lock release so a waker's fast-path load pairs
        // with it (the lost-wake argument in the field's doc block).
        self.waiters.fetch_add(1, Ordering::Release);
        // Lock released here — now waker can see consistent state
    }

    /// Finish wait: remove entry from queue and restore task state to RUNNING.
    ///
    /// Called after schedule() returns (task was woken) or when condition is
    /// already met after prepare_to_wait.
    pub fn finish_wait(&self, task: *mut Task) {
        let mut list = self.list.lock_irqsave();
        let before = list.len();
        list.retain(|entry| entry.task() != task);
        let dropped = before - list.len();
        if dropped > 0 {
            self.waiters.fetch_sub(dropped, Ordering::Release);
        }

        // If task is still sleeping (condition met before schedule, or
        // called from finish path), restore to RUNNING.
        {
            let t = crate::process::r#unsafe::borrow_task(task);
            let state = t.state();
            if state.is_sleeping() {
                t.set_state(super::types::TaskState::new(
                    super::types::TaskState::RUNNING));
            }
        }
    }
}

#[macro_export]
macro_rules! wait_event {
    ($wq_head:expr, $condition:expr) => {{
        let wq_head = $wq_head;
        loop {
            // Check condition
            if $condition {
                break;
            }

            // Condition not met, prepare to wait.
            let current = match crate::sched::hotpath::current() {
                Some(task) => task,
                None => break,
            };

            // Atomically add to wait queue AND set UNINTERRUPTIBLE state
            // (both under the waitqueue lock) to prevent lost-wakeup race.
            wq_head.prepare_to_wait(current, false, false);

            // Re-check condition after prepare_to_wait (state is now UNINTERRUPTIBLE)
            if $condition {
                wq_head.finish_wait(current);
                // R8-5 (NEW-C2): a concurrent wake between prepare_to_wait
                // and here may have enqueued us while we never slept — undo
                // (per-class on_rq guards make it a no-op otherwise).
                crate::sched::hotpath::dequeue_task(&*current);
                break;
            }

            // Yield CPU — task removed from runqueue by __schedule()
            //
            // Enable interrupts before schedule(). We're in syscall context
            // (SIE=0). This ensures lock_irqsave in __schedule saves SIE=1,
            // so when the task is later switched back in, restore_irq restores
            // SIE=1 (via __schedule's unconditional restore_irq(true)).
            crate::arch::cpu::restore_irq(true);
            crate::sched::hotpath::schedule();

            // After wakeup, state is RUNNING (set by enqueue_task_locked)
            // Remove from wait queue and restore state
            wq_head.finish_wait(current);

            // Re-check condition
        }
    }};
}

/// Wait for a condition to become true, interruptible by signals.
///
/// Returns 0 on success (condition met), or -ERESTARTSYS if interrupted by a signal.
#[macro_export]
macro_rules! wait_event_interruptible {
    ($wq_head:expr, $condition:expr) => {{
        let wq_head = $wq_head;
        let _ret = loop {
            // Check condition first (fast path, no locking needed)
            if $condition {
                break 0i32;
            }

            // Check for pending signals
            if crate::signal::signal_pending() {
                break -512i32; // -ERESTARTSYS
            }

            let current = match crate::sched::hotpath::current() {
                Some(task) => task,
                None => break 0i32,
            };

            // Atomically add to wait queue AND set INTERRUPTIBLE (under lock).
            // This prevents the lost-wakeup race where the waker finds the
            // entry on the queue but the task is still RUNNING, marks the
            // entry woken, then the task sets INTERRUPTIBLE and vanishes.
            wq_head.prepare_to_wait(current, false, true);

            // Re-check condition after prepare_to_wait (state is now INTERRUPTIBLE)
            if $condition {
                // Condition met — restore RUNNING and remove from queue
                wq_head.finish_wait(current);
                // R8-5 (NEW-C2): undo a concurrent wake enqueue.
                crate::sched::hotpath::dequeue_task(&*current);
                break 0i32;
            }

            // Re-check signals after prepare_to_wait
            if crate::signal::signal_pending() {
                wq_head.finish_wait(current);
                // R9-17: R8-5 discipline for the signal path too — undo a
                // concurrent wake enqueue before returning.
                crate::sched::hotpath::dequeue_task(&*current);
                break -512i32; // -ERESTARTSYS
            }

            // Enable interrupts before schedule(). We're in syscall context
            // (SIE=0). This ensures lock_irqsave in __schedule saves SIE=1,
            // so when the task is later switched back in, restore_irq restores
            // SIE=1 (via __schedule's unconditional restore_irq(true)).
            crate::arch::cpu::restore_irq(true);
            crate::sched::hotpath::schedule();

            // After wakeup, state is RUNNING (set by enqueue_task_locked).
            // Remove from wait queue.
            wq_head.finish_wait(current);
        };
        _ret
    }};
}


// ════════════════ exit ════════════════════════════════
// (was exit.rs)
// Process/thread exit and wait implementation
//
// - do_exit: termination with thread-group semantics
//   - non-leader thread: ring detach + deferred release (no ZOMBIE —
//     threads are never wait4()ed; Linux delay_put analogue)
//   - leader (or single-threaded): force SIGKILL remaining members,
//     wait for the group to drain, then the classic ZOMBIE flow
// - do_exit_group / de_thread: group teardown entry points (syscall 94,
//   execve, fatal default signals)
// - release_task: Reap zombie child resources (incl. exited group members)
// - do_wait / do_wait_nonblock / do_waitid: wait for child state change

pub fn do_exit(exit_code: i32) -> ! {
    let cur: &mut Task = match crate::sched::hotpath::current() {
        Some(c) => c,
        None => {
            loop {
                // SAFETY: `wfi` is a plain hint instruction with no side effects.
                crate::arch::cpu::wfi();
            }
        }
    };

    // Leg-52 item 0 (fleet s19/s24-r30, stress-lane A/B verdict):
    // release the direct-reclaim gate if THIS task dies while still
    // holding it. The gate's CAS (mm/page_alloc.rs) has exits only at
    // the reclaim block's own returns; a task killed mid-block funnels
    // here without unwinding them, which used to leave IN_DIRECT_RECLAIM
    // stuck at 1 forever — every later allocation then took the
    // no-drain skip path (the fake-OOM family). One CAS on our own pid:
    // a gate already released or held by another task is untouched.
    // SAFETY: current is the raw pointer to the calling task, valid
    // because we are it and do_exit never returns.
    let dying_pid = cur.pid();
    crate::mm::hotpath::reclaim_gate_exit_release(dying_pid);

    // ub-gfx (greeter-chain forensics): name the death of every gdm
    // ladder / accounts-daemon / Xorg process with its exit code, so the
    // serial alone answers "what killed the greeter". Budgeted 120.
    {
        static XIT_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        let n = XIT_N.load(core::sync::atomic::Ordering::Relaxed);
        if n < 120 {
            // SAFETY: current is valid (we are it); comm() reads the
            // task's own fixed comm buffer.
            let comm = cur.comm();
            let name: &[u8] = &comm[..comm.iter().position(|&b| b == 0).unwrap_or(comm.len())];
            let interesting = name.starts_with(b"gdm")
                || name.starts_with(b"Xorg")
                || name.starts_with(b"gnome")
                || name.starts_with(b"accounts")
                || name.starts_with(b"(s-")  // systemd's pre-exec helpers
                || name.starts_with(b"avahi-d");
            if interesting {
                if XIT_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 120 {
                    crate::pr_info!(
                        "GFXEXIT #{} pid={} comm={:?} code={} ({:#x})",
                        XIT_N.load(core::sync::atomic::Ordering::Relaxed),
                        dying_pid,
                        core::str::from_utf8(name).unwrap_or("?"),
                        exit_code,
                        exit_code
                    );
                }
            }
        }
    }

    // R127 wake-drop hunt: the exit-leak detector. A nonzero wq_regs at
    // exit means this task is going away with wait-queue entries still
    // naming it — the park was never unwound (no remove/finish_wait
    // ran). Those entries go stale the moment the Task page is freed
    // and become the pointer-recycling theft / wrong-task-wake class
    // the R127 screen in wait.rs evicts. Gated by the hunt's tracepoint
    // (input:evdev_wake armed via trace_event=); raw-console, budgeted.
    {
        let regs = cur.wq_regs.load(core::sync::atomic::Ordering::Relaxed);
        if regs != 0 && crate::trace::EVENT_EVDEV_WAKE.armed() {
            use core::sync::atomic::{AtomicU32, Ordering as Rel};
            static WQL_N: AtomicU32 = AtomicU32::new(0);
            let n = WQL_N.fetch_add(1, Rel::Relaxed);
            if n < 64 {
                let comm = cur.comm_str();
                let line: alloc::string::String =
                    alloc::format!("WQLEAK[{}] pid={} regs={} comm='{}'\n", n, dying_pid, regs, comm);
                // Raw UART, one write per line: readable at any loglevel.
                // Routed through the taskdump funnel (obs-hygiene): the
                // line stages and flushes atomically instead of per-byte
                // uart_putc_raw, which the OH kmsg flood spliced mid-line
                // on SMP8.
                crate::debug::taskdump::taskdump_raw_line(line.as_bytes());
            }
        }
    }

    // perf EXIT sideband (hiperf process tracking; no-op unless a perf
    // event is armed).
    crate::perf::hook_exit(cur.tgid(), dying_pid);

    // huks-wedge lane (hw2): death-while-holding decode. The wedge's
    // recurring shape ends with a lock-holder thread that VANISHES from
    // every later taskdump while the frozen word keeps its locked value
    // (hw1w1: distributeddata tid 1068 acquired 0x7ff0005f7d94 at
    // 66.8 s and was gone by ~88 s; hw4w1: huks_service tid 477 took a
    // handled stack-boundary SEGV at 9.0 s and was gone by 34 s while
    // two musl __lock words sat at 0x2 forever). The EXIT CAUSE decides
    // the verdict — decode it per holder-class thread:
    //   exit_code < 0            -> death by signal (-exit_code); the
    //                              queued siginfo names the SENDER.
    //   group_exiting && code>=0 -> exit_group(n) (n = code>>8), the
    //                              leader or a fatal group kill.
    //   !leader && !group_exiting-> a single worker/thread exit
    //                              (SYS_exit, status code>>8) — the
    //                              "clean exit that skipped the unlock"
    //                              face, incl. post-SEGV handler unwind
    //                              (see the segv_age field).
    // Fires for: the fuxwatch acquire-latched owner, threads of the
    // park-latched tgid, threads of a tgid with a standing frozen word,
    // and any thread with a delivered-SIGSEGV history. FuxStuck-gated,
    // 96/boot.
    {
        use core::sync::atomic::Ordering::Relaxed as Relaxed2;
        static HWEXIT_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if crate::trace::EVENT_FUXSTUCK.armed()
            && HWEXIT_N.fetch_add(1, Relaxed2) < 96
        {
            let owner_tid = crate::sync::hotpath::FUXWATCH_OWNER_TID.load(Relaxed2);
            let park_tgid = crate::sync::hotpath::FUXPARK_TGID.load(Relaxed2);
            let my_tgid = cur.tgid() as u32;
            let is_leader = dying_pid == my_tgid;
            let frozen = crate::api::sync::rwfrz_tgid_frozen(my_tgid);
            #[cfg(feature = "x86_64")]
            let segv = crate::arch::x86_64::trap::segv_hist_age(dying_pid);
            #[cfg(not(feature = "x86_64"))]
            let segv: Option<(u64, u64)> = None;
            // Edge B (114th): the txnhunt LIVE OWING arm — a thread (or
            // any thread of a tgid) the audit last saw owing a sync
            // binder reply. The frozen-word arm below is blind for the
            // first 60 s of a wedge; this arm catches the silent
            // deaths the 113rd could not see. Snapshot read is
            // lock-free (see binder::txn_owing_of).
            let (owing_n, owing_txn, owing_stid) =
                crate::ipc::binder::txn_owing_of(my_tgid, dying_pid);
            let hit = (owner_tid != 0 && dying_pid == owner_tid)
                || (park_tgid != 0 && my_tgid == park_tgid)
                || (!is_leader && frozen)
                || segv.is_some()
                || owing_n > 0;
            if hit {
                let comm_b = cur.comm();
                let comm = core::str::from_utf8(comm_b.as_slice())
                    .unwrap_or("")
                    .trim_end_matches('\0');
                let lastsys = cur.ext.last_syscall_nr.load(Relaxed2);
                let pending = cur.pending.signal.load(Relaxed2);
                // Group-exit state: distinguishes exit_group/fatal-group
                // (do_exit_group publishes it) from a single thread exit.
                let (group_exiting, _group_code) = cur.signal_arc()
                    .map(|ss| {
                        (
                            ss.group_exiting.load(Relaxed2),
                            ss.group_exit_code.load(Relaxed2),
                        )
                    })
                    .unwrap_or((false, 0));
                let cause = if exit_code < 0 {
                    "signal"
                } else if group_exiting {
                    "exit_group"
                } else if !is_leader {
                    "thread-exit"
                } else {
                    "exit"
                };
                // Sender forensics for signal deaths (kill/tgkill carry
                // si_pid; kernel-originated faults print si_code).
                let (sender, sicode) = if exit_code < 0 {
                    cur.pending
                        .peek_info(-exit_code)
                        .map(|si| (si.si_pid, si.si_code))
                        .unwrap_or((0, 0))
                } else {
                    (0, 0)
                };
                crate::pr_warn!(
                    "HW-EXIT-OF-HOLDER tid={} comm='{}' tgid={} code={:#x} cause={} sig={} sender={} sicode={:#x} leader={} grpexit={} lastsys={} pend={:#x} segv_age_j={} segv_cr2lo={:#x} ownerlatch={} parklatch={} owing_n={} owing_txn={} owing_stid={}",
                    dying_pid,
                    comm,
                    my_tgid,
                    exit_code,
                    cause,
                    if exit_code < 0 { -exit_code } else { 0 },
                    sender,
                    sicode,
                    is_leader,
                    group_exiting,
                    lastsys as i64,
                    pending,
                    segv.map(|s| s.0).unwrap_or(0),
                    segv.map(|s| s.1).unwrap_or(0),
                    owner_tid,
                    park_tgid,
                    owing_n,
                    owing_txn,
                    owing_stid
                );
            }
        }
    }

    // mmi-endgame (116th): the UIEXIT comm arm — clean-death decode for
    // the UI-phase process family. The 115th's boots ended with a
    // system-wide death event at ~136-190 s (hw10w1: ueventd,
    // watchdog_servic, storage_daemon, hdf_devmgr, every *_host,
    // ui_service, render_service, allocator/composer_host,
    // uinput_inject, multimodalinput, bootanimation, then a second wave
    // ending with hilogd) with NO fault rows, NO SIGCHLD kmsg and no
    // reaping — every victim was PARKED (futex/BWR/poll/epoll), so they
    // were killed, not voluntarily exited — and the holder print's
    // owing/frozen/segv arms matched none of them (and its 96-row
    // window is consumed by ALL exits, so it goes blind mid-boot).
    // This arm fires for ANY thread exit whose comm — or whose tgid
    // LEADER's comm — names the UI-phase family, with its OWN budget,
    // and decodes the death: exit code + cause (signal: si_pid names
    // the SENDER; exit_group; thread-exit), the exit CALLER's user
    // return address (pt_regs at do_exit: the instruction after the
    // exit/exit_group syscall site in libc, or the interruption point
    // of a killed thread), the last-syscall latch with args, the parent
    // (pid+comm) that will or will not reap, and the txnhunt owing
    // snapshot. FuxStuck-gated like the holder block above.
    {
        use core::sync::atomic::Ordering::Relaxed as Rel;
        static UIEXIT_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        // early30 lane (120th): + samgr, huks_service — the early-30s
        // intermittent death window's two observed faces (hw10w3 huks
        // SIGABRT 32-34 s; mew3 samgr SIGSEGV 35.26 s) had NO comm arm,
        // so the SIGABRT leader death printed nothing kernel-side (no
        // SEGV history, no owing latch). Same budget, same gating.
        const UIEXIT_COMMS: [&str; 20] = [
            "multimodalinput",
            "uinput_inject",
            "mmi_service",
            "mmi",
            "bootanimation",
            "render_service",
            "composer_host",
            "allocator_host",
            "ui_service",
            "foundation",
            "input_user_host",
            "inputmethod_ser",
            "hdf_devmgr",
            "storage_daemon",
            "ueventd",
            "watchdog_servic",
            "hilogd",
            "sensors",
            "samgr",
            "huks_service",
        ];
        if crate::trace::EVENT_FUXSTUCK.armed()
            && UIEXIT_N.load(Rel) < 96
        {
            let comm_matches = |raw: &[u8; 16]| -> bool {
                let name = core::str::from_utf8(raw).unwrap_or("").trim_end_matches('\0');
                UIEXIT_COMMS.iter().any(|&w| name == w)
            };
            // SAFETY: current is the running (exiting) task; the leader
            // pointer is fixed up by group_leader_ptr() and stays valid
            // until the group is reaped — later than this point.
            let my_comm = cur.comm();
            let leader = cur.group_leader_ptr();
            let leader_comm: &[u8; 16] = if leader.is_null() {
                my_comm
            } else {
                crate::process::r#unsafe::borrow_task(leader).comm()
            };
            let my_named = comm_matches(my_comm);
            if my_named || comm_matches(leader_comm) {
                // Churn filter (first matrix's lesson): the UI processes
                // shed dozens of FFRT worker threads during startup —
                // clean single-thread exit_group(0)-free exits that
                // burned the whole budget in the first 10 s. A thread
                // whose OWN comm is on the list (mmi_service-style
                // named threads) always prints; a match via the LEADER
                // comm alone prints only for non-benign deaths (signal,
                // group exit, leader death, nonzero code).
                let my_tgid_early = cur.tgid() as u32;
                let is_leader_early = dying_pid == my_tgid_early;
                let grp_early = cur
                    .signal_arc()
                    .map(|ss| ss.group_exiting.load(Rel))
                    .unwrap_or(false);
                let benign_churn =
                    !my_named && exit_code == 0 && !grp_early && !is_leader_early;
                if !benign_churn {
                let n = UIEXIT_N.fetch_add(1, Rel);
                if n < 96 {
                    let comm = core::str::from_utf8(my_comm)
                        .unwrap_or("")
                        .trim_end_matches('\0');
                    let leader_name = core::str::from_utf8(leader_comm)
                        .unwrap_or("")
                        .trim_end_matches('\0');
                    let my_tgid = cur.tgid() as u32;
                    let is_leader = dying_pid == my_tgid;
                    // Parent at death time (pid + comm for the reaping
                    // question). None/implausible = racing reaper or an
                    // already-freed parent pointer.
                    let (ppid, pcomm) = match cur.parent_ptr() {
                        Some(pp) if Task::is_plausible_task_ptr(pp) => {
                            let pp_ref = crate::process::r#unsafe::borrow_task(pp as *mut Task);
                            (
                                pp_ref.pid(),
                                alloc::string::String::from(
                                    core::str::from_utf8(pp_ref.comm())
                                        .unwrap_or("")
                                        .trim_end_matches('\0'),
                                ),
                            )
                        },
                        _ => (0, alloc::string::String::from("?")),
                    };
                    let lastsys = cur.ext.last_syscall_nr.load(Rel) as i64;
                    let la0 = cur.ext.last_syscall_a0.load(Rel);
                    let la1 = cur.ext.last_syscall_a1.load(Rel);
                    let (group_exiting, _group_code) = cur.signal_arc()
                        .map(|ss| {
                            (ss.group_exiting.load(Rel), ss.group_exit_code.load(Rel))
                        })
                        .unwrap_or((false, 0));
                    let cause = if exit_code < 0 {
                        "signal"
                    } else if group_exiting {
                        "exit_group"
                    } else if !is_leader {
                        "thread-exit"
                    } else {
                        "exit"
                    };
                    // Sender forensics for signal deaths (kill/tgkill
                    // carry si_pid; kernel-forced kills print si_code).
                    let (sender, sicode) = if exit_code < 0 {
                        cur.pending
                            .peek_info(-exit_code)
                            .map(|si| (si.si_pid, si.si_code))
                            .unwrap_or((0, 0))
                    } else {
                        (0, 0)
                    };
                    // The exit caller's user return address (see the
                    // RWFRZ-THREADEXIT precedent above for the cfg split).
                    let exrip = match cur.pt_regs_ref() {
                        Some(pr) => crate::process::r#unsafe::pt_user_pc(pr),
                        None => 0u64,
                    };
                    let (owing_n, owing_txn, owing_stid) =
                        crate::ipc::binder::txn_owing_of(my_tgid, dying_pid);
                    // wrl-class lane: owing_txn without owing_stid conflates
                    // the thread's OWN debt with the tgid-wide fallback —
                    // txn_owing_of returns the first txn owed by ANY thread
                    // of the tgid when this tid has no exact match (f2's
                    // tid-1523 exit printed owing txn4570, which its live
                    // sibling 975 owed; the neverword lane read that as
                    // "handler exits mid-transaction"). own=1 marks the
                    // exact-tid match; own=0 means a sibling owes it.
                    let owing_own = owing_stid != 0 && owing_stid == dying_pid;
                    // HOTPATH-PRINT: rux:fuxstuck (retired debug=fuxstuck)-gated UI-family exits.
                    crate::pr_warn!(
                        "UIEXIT[{}] tid={} comm='{}' tgid={} leader='{}' ppid={}('{}') code={:#x} cause={} sig={} sender={} sicode={:#x} grpexit={} leader={} lastsys={}({:#x},{:#x}) exrip={:#x} owing_n={} owing_txn={} owing_stid={} own={}",
                        n,
                        dying_pid,
                        comm,
                        my_tgid,
                        leader_name,
                        ppid,
                        pcomm,
                        exit_code,
                        cause,
                        if exit_code < 0 { -exit_code } else { 0 },
                        sender,
                        sicode,
                        group_exiting,
                        is_leader,
                        lastsys,
                        la0,
                        la1,
                        exrip,
                        owing_n,
                        owing_txn,
                        owing_stid,
                        if owing_own { 1 } else { 0 }
                    );
                }
                }
            }
        }
    }

    // sc57 (57th session): thread-exit forensics for the OH wedge —
    // an FFRT/binder worker that exits while holding a userspace
    // ffrt::mutex leaves every later locker parked forever with no
    // kernel-visible holder (the mutex has no owner field in the
    // image's libffrt). rux:txnhunt (retired debug=txnhunt) prints every NON-LEADER exit in
    // the registered samgr pid, bounded to 64/boot.
    {
        use core::sync::atomic::Ordering::Relaxed;
        static THEXIT_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        let tgid = cur.tgid();
        let is_leader = dying_pid == tgid;
        if !is_leader
            && crate::trace::EVENT_TXNHANG.armed()
            && tgid != 0
            && tgid == crate::ipc::binder::binder_sareg_samgr_pid()
            && THEXIT_N.fetch_add(1, Relaxed) < 64
        {
            let comm = core::str::from_utf8(cur.comm())
                .unwrap_or("")
                .trim_end_matches('\0');
            // HOTPATH-PRINT: rux:txnhunt (retired debug=txnhunt)-gated samgr thread exit line.
            crate::pr_warn!(
                "SAMGR-THREADEXIT tid={} comm='{}' code={}",
                dying_pid,
                comm,
                exit_code
            );
        }
        // rwlock-livelock: thread-exit forensics — the ub2404 wedge's
        // frozen word is GLib's GAsyncQueue mutex (pool-spawner parked
        // in g_cond_wait's relock at word=2) and the mutex holder
        // matches NO live task. If a thread EXITS while holding the
        // queue mutex (glibc normal mutexes are not robust; GLib's
        // spawn protocol has no ownership recovery), the wedge is a
        // userspace death-while-holding. Gate on the tgid of the FIRST
        // process that parks a waiter on a word the census later flags
        // (learned dynamically — comm-gating missed GLib's
        // "pool-udisksd" worker comms in boot J). NON-LEADER exits only
        // (thread exits; whole-process death is not this shape).
        {
            use core::sync::atomic::Ordering::Relaxed;
            static UDEXIT_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let watch_tgid = crate::api::sync::rwfz_exit_watch_tgid();
            let is_leader = dying_pid == tgid;
            if !is_leader
                && watch_tgid != 0
                && tgid == watch_tgid
                && UDEXIT_N.fetch_add(1, Relaxed) < 128
            {
                let comm_b = cur.comm();
                let comm = core::str::from_utf8(comm_b.as_slice())
                    .unwrap_or("")
                    .trim_end_matches('\0');
                let lastsys = cur.ext.last_syscall_nr.load(Relaxed);
                let urip = match cur.pt_regs_ref() {
                    Some(pr) => crate::process::r#unsafe::pt_user_pc(pr),
                    None => 0u64,
                };
                // HOTPATH-PRINT: thread exits of the watched tgid.
                crate::pr_warn!(
                    "RWFRZ-THREADEXIT tid={} comm='{}' code={} lastsys={} unrip={:#x}",
                    dying_pid,
                    comm,
                    exit_code,
                    lastsys as i64,
                    urip
                );
                // rwlock-livelock: the dying thread's USER return chain —
                // the exit rip is always libc's exit syscall site and
                // cannot say WHERE the thread came from; the live frames
                // below it name the GLib path (worker-proxy retirement vs
                // a requester dying between g_async_queue_lock and
                // g_cond_wait — the death-while-holding window).
                if UDEXIT_N.load(Relaxed) <= 8 {
                    // SAFETY: current is the calling task; read-only walk
                    // of its own user stack (the frames are intact at
                    // do_exit time).
                    crate::process::r#unsafe::apppark_rets(cur);
                }
            }
        }
    }
    // 50th (crash-storm hunt): failed-spawn forensics — a process LEADER
    // that exits without ever having execve'd forked and died pre-exec
    // (the OH 337->588->589 chain: 588 exits 512/code 2, 589 exits 0,
    // zero execve attempts anywhere in their rings). The birth window
    // (first 64 syscalls of the task's life, latched at dispatch) names
    // the killing call. Threads are skipped (they never exec); prints
    // are bounded to 8 per boot — kernel-thread exits and subshell-style
    // fork-and-exit children burn at most the budget, never the serial.
    {
        use core::sync::atomic::Ordering::Relaxed;
        static NOSPWN_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        let is_leader = cur.pid() == cur.tgid();
        let execd = cur.ext.did_exec.load(Relaxed);
        if is_leader && !execd && NOSPWN_N.fetch_add(1, Relaxed) < 8 {
            let comm = core::str::from_utf8(cur.comm())
                .unwrap_or("")
                .trim_end_matches('\0');
            let mut line = alloc::string::String::from("NOSPAWN-EXIT pid=");
            use core::fmt::Write;
            let _ = write!(
                line,
                "{} comm='{}' code={} birth:",
                dying_pid, comm, exit_code
            );
            for i in 0..super::types::BIRTH_CALLS_LEN {
                let nr = cur.ext.birth_calls[i].load(Relaxed);
                if nr == 0 {
                    break; // ring not filled past here
                }
                let a0 = cur.ext.birth_a0[i].load(Relaxed);
                let _ = write!(line, " {}:{:#x}", nr & 0xffff, a0);
            }
            crate::pr_debug!("{}", line);
        }
    }

    // forkprof (cmdline forkprof=1): leader-arm phase timings. The probe
    // rig's children are single-threaded, so the leader arm is the exit
    // cost lmbench attributes to fork+exit.
    let fp = crate::debug::forkprof::on() && dying_pid == cur.tgid();
    let fp_t0 = if fp { crate::debug::forkprof::now() } else { 0 };
    if fp {
        crate::debug::forkprof::trace(crate::debug::forkprof::T_EXIT_ENTRY, dying_pid);
    }

    // The sched:sched_process_exit tracepoint, recorded at do_exit entry
    // (the reference's record point). group_dead: this exit takes the
    // thread group to its last live thread — nr_threads has not been
    // decremented yet, so 1 means this is it. Gated behind armed().
    if crate::trace::EVENT_SCHED_PROCESS_EXIT.armed() {
        // cur is the exiting task, pinned by being the running context.
        {
            let words = crate::trace::comm_to_words(cur.comm());
            crate::trace::record(
                &crate::trace::EVENT_SCHED_PROCESS_EXIT,
                [
                    words[0],
                    words[1],
                    cur.pid() as u64,
                    cur.prio() as u64,
                    (cur.nr_threads() <= 1) as u64,
                    0,
                    0,
                    0,
                    0,
                    0,
                ],
            );
        }
    }

    // current is the raw pointer to the calling task, guaranteed valid
    // since we are the currently executing task and will never return
    // from this function. One consolidated reference.
    // SAFETY: we are `current`; do_exit never returns.

    {
        // sc129 (Session P): the OH app children's FFRT worker threads are
        // CREATED (clone CLONE_THREAD succeeds, census pid logged) and then
        // exit instantly and CLEANLY (process survives) — the worker's own
        // fail-latch dies with it. Print the exiting THREAD's fail tail
        // here (leader exits excluded — those are whole-process exits with
        // their own forensics). Budget 12.
        //
        // Serial-clean (perf-fork lane, 2026-11): the whole block — two
        // ring-dump Strings plus a ~500-char pr_warn line — cost ~6 ms of
        // polled-16550 UART time per thread exit and ran for the first 80
        // thread exits of every boot; pthread create/exit loops (and any
        // thread-heavy bring-up) paid it directly. The dumps belong to
        // the syscall-trace hunt that added them: default off behind
        // rux:syscalltrace (retired debug=syscalltrace) (sc129/sc131/sc135 hunts re-arm with the flag).
        if !(cur.pid() == cur.tgid())
            && crate::trace::EVENT_SYSCALLTRACE.armed()
        {
            use core::fmt::Write;
            let mut s = alloc::string::String::new();
            {
                let cur8 =
                    cur.ext.last_fails_cur.load(core::sync::atomic::Ordering::Relaxed);
                for k in 0..8 {
                    let i = (cur8 + 8 - 1 - k) % 8; // newest first
                    let v = cur.ext.last_fails[i].load(core::sync::atomic::Ordering::Relaxed);
                    if v != 0 {
                        // sc135 (Session Q): a0/a1 name the failing call's
                        // leading args (prctl option et al).
                        let a0 = cur.ext.last_fails_a0[i]
                            .load(core::sync::atomic::Ordering::Relaxed);
                        let a1 = cur.ext.last_fails_a1[i]
                            .load(core::sync::atomic::Ordering::Relaxed);
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
            }
            // sc131 (Session P): the FFRT worker batch exits CLEANLY with
            // ZERO failing syscalls — the fails-only print was silent for
            // exactly the threads we hunt. Always print (threads only),
            // now with the ALL-calls ring (96 since sc135, newest first)
            // so the worker's whole short life is visible.
            let mut c = alloc::string::String::new();
            {
                const RL: usize = super::types::LAST_CALLS_LEN;
                let cur12 =
                    cur.ext.last_calls_cur.load(core::sync::atomic::Ordering::Relaxed);
                for k in 0..RL {
                    let i = (cur12 + RL - 1 - k) % RL; // newest first
                    let v = cur.ext.last_calls[i].load(core::sync::atomic::Ordering::Relaxed);
                    if v != 0 {
                        let _ = write!(c, " {}", v);
                    }
                }
            }
            static NW: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if NW.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 80 {
                let nc = core::str::from_utf8(cur.comm())
                    .unwrap_or("")
                    .trim_end_matches('\0');
                // SAFETY: the leader is a live ring member (group not
                // yet torn down at this point of do_exit).
                let lc = core::str::from_utf8(
                    crate::process::r#unsafe::borrow_task(cur.group_leader_ptr()).comm(),
                )
                .unwrap_or("")
                .trim_end_matches('\0');
                crate::pr_warn!(
                    "thead-exit: tid {} comm='{}' leader='{}' code={} calls:{} fails:{}",
                    cur.pid(),
                    nc,
                    lc,
                    exit_code,
                    c,
                    s
                );
            }
        }
        // Linux discipline: PF_EXITING is do_exit's FIRST store, so every
        // other subsystem (the scheduler's R56/R57 harvest-steal machinery
        // in particular) can refuse to "rescue" a task whose kernel stack
        // is live in the exit path — re-enqueueing it hands that stack to
        // a second hart (the n1-organic double-run panic).
        cur.set_ti_flags(
            cur.ti_flags()
                | super::types::TaskFlags::PF_EXITING.bits(),
        );
        let current_pid = cur.pid();
        let leader = cur.group_leader_ptr();
        let is_leader = leader as *const Task == cur as *const Task;
        let parent_pid = if is_leader {
            cur.ppid()
        } else {
            // Threads share the leader's real parent; the leader was
            // resolved from the ring above and is still live.
            crate::process::r#unsafe::borrow_task(leader).ppid()
        };

        // ===== Journal handle unwind =====
        // A task killed mid-syscall (SIGKILL/SIGSEGV between
        // set_current_handle and clear_current_handle — LTP abort01 sends
        // SIGABRT into its children) dies owning a jbd2 handle: its
        // transaction's t_updates stays elevated forever, and EVERY later
        // jbd2_journal_stop spins out its 1,000,000-lap "wait for other
        // handles" loop (seconds under TCG — the capset01/03 soft-lockup
        // family; multiplied per write it became a whole-chunk wedge).
        // Unwind the reference now: the handle lives on this task's dying
        // stack and can never reach ext4_journal_stop itself. The
        // registered dirty buffers stay in the transaction for the next
        // stopper to commit; nothing else in the handle needs release.
        // The current-task handle owns this journalling slot; the
        // edges document their per-task exclusivity (INV-PROC-JOURNAL-1).
        {
            // The rung-5 seam row: the handle type is ext4-crate-owned;
            // the row body does the refcount drop + slot clear
            // (INV-PROC-JOURNAL-1 documented crate-side).
            crate::api::ext4::exit_journal_unwind();
        }

        // ===== Ext4 big lock force-release =====
        // Same leak family as the journal handle above: do_exit does not
        // unwind Rust guards, so a task killed inside an ext4 critical
        // section would strand EXT4_BIG_LOCK on a dead owner forever
        // (GNOME r72-fire2: every CPU then spun or slept against the
        // dead owner's pid — even a recycled one — while gs-b/Xorg
        // starved). The dying task can make no forward progress in its
        // section; drop the lock and wake all waiters so the filesystem
        // survives the kill.
        crate::fs::hotpath::EXT4_BIG_LOCK.release_if_owner(current_pid);

        // ===== VFS mutation lock force-release =====
        // Same leak family: do_exit does not unwind Rust guards, so a
        // task killed inside a lookup+mutate window would strand
        // VFS_MUTATION_LOCK (a SleepMutex since the sync-realign lane)
        // on a dead owner forever.
        crate::fs::hotpath::VFS_MUTATION_LOCK.release_if_owner(current_pid);

        // ===== Ptrace tracer death: auto-detach tracees =====
        // Linux forgets the tracing relationship when the tracer exits
        // (ptrace_exit → every tracee is detached and resumed). Without
        // this, a tracer that dies between PTRACE_ATTACH and DETACH (the
        // LTP runner 30s-kills a wedged test) leaves its tracee STOPPED
        // forever — when the tracee is init (ptrace11 attaches PID 1) the
        // whole guest wedges.
        {
            super::hotpath::pid_hash_for_each_task(|t| {
                if (*t).tracer_pid() != current_pid || (*t).pid() == current_pid {
                    return;
                }
                // SAFETY: task is alive for the duration of the hash walk
                // (same discipline as send_signal_to_pgid's in-walk use).
                t.set_tracer_pid(0);
                t.set_ptrace_options(0);
                t.clear_single_step();
                if t.state().contains(TaskState::STOPPED) {
                    crate::signal::signal_wake_up(t as *const Task as *mut Task);
                }
            });
        }

        // ===== Core dump (P1) =====
        // A negative exit code encodes death by signal; the core-dumping
        // signals get a core file written BEFORE any resource is torn
        // ===== Binder pool reaping =====
        // Threads killed by a signal never reach BINDER_THREAD_EXIT;
        // without this their pool entries live forever, the spawner's
        // threads.len() gate counts corpses and never replenishes, and
        // the process's clients stall (b92: samgr proc todo depth 13,
        // hdf_devhost waiters parked 162s). Cleanly-exiting IPC threads
        // also route through the same reaper via the ioctl.
        crate::ipc::binder::binder_task_died(
            current_pid,
            cur.tgid(),
            cur.ns_pid_local(),
        );

        // ===== Core dump (P1) =====
        // down. Runs in the dying task's context (its cwd/fdtable/mm are
        // still intact). This is the single funnel for both the signal
        // default-action path (handle_default_signal -> do_exit_group)
        // and the direct trap paths (SIGSEGV/SIGILL/... -> do_exit).
        if exit_code < 0 && crate::api::process::signal_makes_core(-exit_code) {
            // cur is the exiting task (do_coredump's contract; it never
            // runs twice for one task).
            crate::api::process::do_coredump(cur, -exit_code);
        }

        crate::pr_debug!("exit: pid={}, tgid={}, exit_code={}, ppid={} ({})",
            current_pid, cur.tgid(), exit_code, parent_pid,
            if is_leader { "leader" } else { "thread" });

        // DEBUG (dbus-death hunt): every death-by-signal gets a KERN_INFO
        // line on the console — the default loglevel hides pr_debug, and
        // the only external symptom of the dbus-daemon kill was
        // ECONNREFUSED with the socket file left behind. `kill -0`-style
        // exits (normal, code>=0) are not printed: the fork/exec storm
        // would bury the signal deaths in noise.
        //
        // RETIRED (probe-retire): the line was unconditional (18 lines
        // per ub2404 boot — every service that died of a signal). It now
        // lives behind the existing signal-family switch (rux:sigtrace (retired debug=sigtrace)).
        if exit_code < 0
            && crate::trace::EVENT_SIGTRACE.armed()
        {
            let comm_raw = cur.comm();
            let comm_len = comm_raw.iter().position(|&b| b == 0).unwrap_or(comm_raw.len());
            // Sender forensics: SI_USER deaths (kill/tgkill) carry the
            // sender's pid in the queued siginfo — print it so "who killed
            // X" is answerable from the console (kernel-originated signals
            // have no sender and print si_code instead).
            let sender = cur.pending
                .peek_info(-exit_code)
                .map(|si| (si.si_pid, si.si_code))
                .unwrap_or((0, 0));
            crate::pr_info!(
                "SIGDEATH: pid={} tgid={} comm={:?} sig={} ppid={}{} si_pid={} si_code={:#x}",
                current_pid,
                cur.tgid(),
                core::str::from_utf8(&comm_raw[..comm_len]),
                -exit_code,
                parent_pid,
                if is_leader { "" } else { " (thread)" },
                sender.0,
                sender.1,
            );
        }

        #[cfg(feature = "debug-futex-trace")]
        {
            use crate::debug::taskdump::{taskdump_dec, taskdump_raw_line};
            taskdump_raw_line(b"FTX EXIT pid=");
            taskdump_dec(current_pid as u64);
            taskdump_raw_line(b" code=");
            taskdump_dec(exit_code as i64 as u64);
            taskdump_raw_line(b" threads=");
            taskdump_dec(cur.nr_threads() as u64);
            taskdump_raw_line(b"\n");
        }

        // Set exit code
        cur.set_exit_code(exit_code);

        // ===== CPU-time accounting fold =====
        // Linux semantics: a dying thread's user/system ticks fold into the
        // thread-group leader (so /proc/[pid]/stat, times() and
        // getrusage(RUSAGE_SELF) report group totals), and the leader's
        // accumulated totals fold into the PARENT's cutime/cstime (what
        // times()/getrusage(RUSAGE_CHILDREN) report after wait()). Lost
        // silently when the parent already exited (orphan), matching the
        // fact that nobody can wait for them anymore.
        {
            use core::sync::atomic::Ordering::Relaxed;
            let u = cur.utime_ticks.load(Relaxed);
            let s = cur.stime_ticks.load(Relaxed);
            // ru_maxrss propagation (Linux signal->cmaxrss): the child's
            // resident set at exit (captured below while the mm is still
            // alive) folds into the parent's RUSAGE_CHILDREN hiwater.
            let rss_kb = {
                let mm_rss = cur.address_space()
                    .map(|mm| mm.rss())
                    .unwrap_or(0);
                mm_rss * (crate::mm::types::PAGE_SIZE / 1024) as u64
            };
            cur.ext.exit_maxrss_kb.store(rss_kb, Relaxed);
            if is_leader {
                // RUSAGE_CHILDREN folds at REAP time, not exit time
                // (reference wait_task_zombie accumulates into the
                // parent's signal struct only when the zombie is
                // waited): a parent reading RUSAGE_CHILDREN while the
                // child is still a ZOMBIE must NOT see its contribution
                // (LTP getrusage03's pre_wait assertion). The fold lives
                // in release_task; exit only captures exit_maxrss_kb.
            } else {
                // The leader is alive (we are its member; the ring
                // decrement happens below).
                let leader_ref = crate::process::r#unsafe::borrow_task(leader);
                leader_ref.utime_ticks.fetch_add(u, Relaxed);
                leader_ref.stime_ticks.fetch_add(s, Relaxed);
            }
        }

        // Drop any in-progress signal frame: a handler that longjmp'd out
        // left sigframe set, which blocks all further delivery including
        // SIGKILL (review H15) — clear it so the dying task stays killable.
        cur.sigframe = None;
        cur.sigframe_addr = 0;

        // ===== Robust list + clear_child_tid: BEFORE the mm ref drops =====
        // Both write into user memory and futex-wake with THIS task's mm
        // as the key identity; once set_address_space(None) runs the key
        // is unobtainable and the user words unreachable.
        crate::api::process::exit_robust_list(cur);
        crate::api::process::exit_clear_child_tid(cur);

        // ===== cgroup v2 (U1b): leave the cgroup + settle memory =====
        // Drops cgroup membership (pids.current / cgroup.procs) and
        // uncharges the task's mmap(2) memory ledger — see
        // sched::cgroup::cgroup_exit for why the ledger, not total_vm,
        // is the uncharge unit.
        crate::sched::hotpath::cgroup_exit(cur);

        // ===== exit_mm: drop THIS task's mm reference =====
        // Linux exit_mm discipline:
        // - shm detach (nattch) runs inside MmStruct::drop now — exactly
        //   once per mm, when the LAST Arc reference goes away. The old
        //   pre-drop `Arc::strong_count == 1` check here was racy against
        //   walker-held clones (ptrace/OOM/rmap) and against concurrent
        //   sibling exits (double detach / missed detach).
        // - satp switch to the kernel root page table: EVERY dying task on
        //   a user page table, last or not. After clear_child_tid/robust
        //   list nothing touches user memory, and a dying group member
        //   holding the last reference FREES the page tables while this
        //   task may still be spinning in wait_for_thread_group_death on
        //   another CPU — running on a freed root is the ARCH-H4 fault.
        // - set_address_space(None) swaps the slot under the mm-slot lock
        //   and drops our Arc after it; if it is the last reference the
        //   teardown (grace wait + PT free + VMA release) runs in THIS
        //   context, after the satp switch. SysV shm detach (nattch) runs
        //   inside MmStruct::drop — exactly once per mm, which subsumes
        //   the old strong_count==1 pre-check here (LTP shmat01 family).
        if cur.address_space_arc().is_some() {
            crate::process::r#unsafe::switch_to_kernel_root_on_exit();
        }
        let fp_t_mmdrop = if fp { crate::debug::forkprof::now() } else { 0 };
        cur.set_address_space(None);
        cur.clear_active_mm();
        // (fork-frag lane) exit is the other natural table-churn point:
        // drain defer-freed leaf tables so the frames return before the
        // next fork's child-table allocs look for them (no-op on arches
        // without the protocol).
        crate::arch::mm::pt_pending_drain();
        if fp {
            crate::debug::forkprof::add(crate::debug::forkprof::P_EX_MMDROP, fp_t_mmdrop);
            crate::debug::forkprof::trace(crate::debug::forkprof::T_MMDROP_DONE, dying_pid);
        }

        // Wake vfork parent (if any) — child has dropped the shared address
        // space, so the parent can safely resume.  This must happen before
        // ZOMBIE state so that the parent sees the child is still valid.
        super::hotpath::vfork_wake_parent(cur);

        // ===== exit_files: Release file descriptor table reference =====
        cur.set_fdtable(None);

        // ===== Clean up futex waiters =====
        crate::sync::hotpath::futex_cleanup(cur);
        // PI futexes: release held locks (owner-died handoff to the top
        // waiter) and leave every PI wait queue.
        crate::sync::hotpath::futex_pi_exit_cleanup(cur);

        // ===== Clean up POSIX MQ fd entries =====
        crate::api::ipc::mq_fds_cleanup(cur);

        // ===== Disarm interval timers (ITIMER_REAL/VIRTUAL/PROF) =====
        for i in 0..3 {
            let old_id = cur.itimer_ids[i].swap(0, core::sync::atomic::Ordering::AcqRel);
            if old_id != 0 {
                crate::timer::del_timer(old_id);
            }
        }

        // ===== Disarm POSIX timers =====
        {
            let mut timers = cur.posix_timers.lock();
            for pt in timers.drain(..) {
                if pt.kernel_timer_id != 0 {
                    crate::timer::del_timer(pt.kernel_timer_id);
                }
            }
        }

        // ===== Reverse SEM_UNDO adjustments =====
        crate::ipc::sysv_sem::sem_undo_exit(cur);

        // ===== Orphan reparenting (review PROC-P03) =====
        // Children of a dying task must be re-attached (a surviving
        // thread-group member first, else the subreaper/init (PID 1)),
        // otherwise their parent pointers dangle after our Task slot is
        // freed (ppid()/for_each_child UAF) and orphaned ZOMBIEs are
        // never reaped — the PID space leaks monotonically.
        // Applies to threads too: a thread may have fork()ed children.
        // Process-tree mutations take PROCESS_TREE_LOCK inside.
        let fp_t_rep = if fp { crate::debug::forkprof::now() } else { 0 };
        crate::api::process::reparent_children_to_init(cur);
        if fp {
            crate::debug::forkprof::add(crate::debug::forkprof::P_EX_REPARENT, fp_t_rep);
        }

        // ===== Thread-group split =====
        if !is_leader {
            // ---- Non-leader thread: deferred release, never a ZOMBIE ----
            // Our mm Arc reference is already dropped above; other members
            // keep the address space alive.
            cur.signal = None; // shared with the leader; drop our ref
            cur.set_fs(None); // CLONE_FS-shared; drop our ref

            // Park on the leader's dead list BEFORE leaving the ring.
            // Ordering matters: nr_threads hits 1 in thread_group_leave,
            // which unblocks a dying leader (and the parent's reap of it).
            // If the push came after the leave, the leader could be fully
            // released in that window and we would push onto freed memory.
            // Parking first is safe: free_dead_member spins on on_cpu,
            // which stays set until our final schedule below — so the
            // leader's release cannot pass our entry while we still touch
            // leader fields.
            // SAFETY: leader comes from group_leader_ptr() while we are
            // still linked in its ring — it cannot be mid-release (its
            // release_task waits for ring members to be DEAD first).
            if !leader.is_null() {
                // SAFETY: the leader is alive; the list is lock-guarded.
                crate::process::r#unsafe::borrow_task(leader).dead_threads.lock().push(cur as *mut Task);
            }

            // Leave the ring (decrements the leader's nr_threads) — AFTER
            // the mm drop and the dead-list park.
            let leader = Task::thread_group_leave(cur);

            // r16 (mechanism B, re-landed on the RELEASE-DEFER drain): the
            // sweep below can SLEEP — free_dead_member ->
            // synchronize_rcu_bounded_pollfirst/_sleepy arms a PID-keyed
            // one-shot timer wake for every 1-jiffy poll round (and the
            // deferred-release finisher at the sweep's tail parks the same
            // way). Freeing our pid BEFORE the sweep made each such wake
            // unresolvable: the expiry hit a freed (possibly reused) pid,
            // WAKE-PID-MISS dropped the token, and the waiter parked past
            // its own deadline — the bound is only re-checked on wake, so
            // the "bounded" wait became a permanent mid-exit sleep (the GB
            // worker wedges; the OH SIGKILL storms reproduce the class).
            // The hash remove + free now run AFTER the sweep: the tid
            // lingers only across the sweep, the task never returns to
            // userspace, and a tgkill/gettid racing this window observes a
            // PF_EXITING task — the reference's own pre-reap shape.
            // rcu.rs keeps an independent guard (RCU-SLEEPY-PID-FREE) for
            // any future pid-then-sleep order.

            // Opportunistically reclaim older dead members (frees stacks of
            // previously-joined threads; `current` is skipped — on_cpu).
            if !leader.is_null() {
                crate::api::process::sweep_dead_threads(leader);
            }

            // PID + hash bookkeeping: the tid disappears now (gettid/tgkill
            // on it → ESRCH, no zombie window).
            super::hotpath::pid_hash_remove(current_pid);
            crate::api::process::free_pid(current_pid);

            // R9-3 discipline: no preempt window between DEAD and the
            // final schedule.
            crate::interrupt::preempt::preempt_count_add(1);
            // Final state: DEAD (not ZOMBIE — wait4 never sees threads).
            cur.set_state(TaskState::new(TaskState::DEAD));
            crate::sched::hotpath::dequeue_task(cur);
            crate::interrupt::preempt::preempt_count_sub(1);

            // ===== do_task_dead: Final schedule, never returns =====
            // sc186/sc189 (session R eighth leg): the plain schedule()
            // could be repicked/refused (a racing wake/heal re-links the
            // exiting task; __schedule's same-task repick fast path then
            // returns without switching) — the wfi park loop below would
            // strand this CPU forever with the slot still naming the dead
            // task and its leaked preempt bits uncleared (PREEMPT-LEAK
            // pc=0x10000 rip=wfi+5, 10+ min, twice reproduced).
            // schedule_dead_never_returns() re-asserts DEAD + unlinks +
            // clears leaked preempt bits until a real context switch.
            crate::sched::hotpath::schedule_dead_never_returns();

            loop {
                // SAFETY: `wfi` is a plain hint instruction with no side effects.
                crate::arch::cpu::wfi();
            }
        }

        // ---- Leader (or single-threaded process): classic ZOMBIE flow ----
        // If group members are still alive (exit() from the leader, or
        // exit_group), kill them and wait for the drain before tearing
        // down the shared-mm-dependent state any further.
        if cur.nr_threads() > 1 {
            crate::api::process::kill_other_threads(cur);
            // NOT killable: an exiting leader cannot unwind, and the
            // shared-mm teardown below requires the drain (see the
            // wait_for_thread_group_death doc).
            crate::api::process::wait_for_thread_group_death(cur, false);
        }

        // ===== pid-namespace init exit: tear the namespace down =====
        // zap_pid_ns_processes parity: when a container's init dies, every
        // task left in that namespace — and in DESCENDANT namespaces (they
        // hold pids in it too) — receives a kernel-forced SIGKILL. Without
        // the cascade the survivors lingered under an unreachable pid-1
        // (LTP pidns05: the whole nested chain must die with the outer
        // init). Kernel-origin SIGKILL pierces the unkillable-init rule in
        // do_signal (the SEND_SIG_PRIV force exception), so nested
        // container inits die too.
        if cur.ns_pid.is_some() && cur.ns_local_pid == 1 {
            let dying_ns = cur.ns_pid.clone();
            // gdm bring-up r9/r10: a silent mass SIGKILL took out the
            // whole daemon set (dbus, upowerd, accounts, tailer, sink,
            // polkitd, gdm3) with ZERO prints at any loglevel and no
            // SIGKILL-SEND trace — this loop is the only silent kernel
            // SIGKILL fan-out. Name the namespace init and every victim
            // until this is root-caused.
            // SAFETY: current is the raw pointer to the calling task,
            // valid because we are it.
            let ccomm = alloc::string::String::from(cur.comm_str());
            let ns_addr = dying_ns
                .as_ref()
                .map(|d| alloc::sync::Arc::as_ref(d) as *const _ as usize)
                .unwrap_or(0);
            let mut victims: alloc::vec::Vec<(u32, alloc::string::String)> =
                alloc::vec::Vec::new();
            super::hotpath::pid_hash_for_each_task(|task| {
                let t = &*task;
                if t.pid() == current_pid {
                    return;
                }
                let hit = dying_ns.as_ref().map(|d| {
                    // Same namespace or any descendant: walk the target's
                    // parent chain looking for the dying namespace.
                    let mut ns = t.ns_pid.clone();
                    while let Some(n) = ns {
                        if alloc::sync::Arc::ptr_eq(&n, d) {
                            return true;
                        }
                        ns = n.parent.clone();
                    }
                    false
                }).unwrap_or(false);
                if hit {
                    victims.push((
                        t.pid(),
                        alloc::string::String::from(
                            t.comm_str(),
                        ),
                    ));
                }
            });
            crate::pr_err!(
                "PIDNS-KILL: ns-init pid={} comm='{}' ns=0x{:x} victims={}",
                current_pid,
                ccomm,
                ns_addr,
                victims.len()
            );
            for (vp, vc) in victims.iter().take(20) {
                crate::pr_err!("PIDNS-KILL victim pid={} comm='{}'", vp, vc);
            }
            for (vp, _) in victims.iter() {
                let _ = crate::signal::send_signal(
                    *vp,
                    crate::signal::Signal::SIGKILL as i32,
                );
            }
        }

        // ===== PID 1 exit: power the machine down =====
        // No orphan is ever reaped or respawned without init — the
        // shutdown(8) init-signal path lands here (init kills every user
        // process, including shutdown(8) itself, then exits). Runs the
        // reboot(2) cascade (never returns) unless one is already in
        // flight — that caller is waiting for exactly this exit.
        if current_pid == 1 {
            crate::api::process::init_exited_fallback_cascade();
        }

        // R9-3: close the preempt window between ZOMBIE and the deferred
        // notify — a timer IRQ landing here would schedule() us out with
        // state != RUNNING, and lines below (the notify arm) would never
        // run: the parent's only wake source lost.
        let fp_t_notify = if fp { crate::debug::forkprof::now() } else { 0 };
        crate::interrupt::preempt::preempt_count_add(1);
        // Set process state to Zombie
        cur.set_state(TaskState::new(TaskState::ZOMBIE));

        // Dequeue from global run queue
        crate::sched::hotpath::dequeue_task(cur);

        // pidfd exit notification: every pidfd poller targeting this
        // thread group must observe EPOLLIN now (the reference wakes the
        // pid's waitqueue here). The woken tasks re-check their own
        // target's state, so the global queue's spurious wakes are
        // harmless; this CPU holds preempt_count, so they run after the
        // terminal schedule below.
        crate::fs::hotpath::exit_notify_all_waiters();

        // ===== exit_notify: Defer parent notification =====
        // CRITICAL: Do NOT wake the parent here.  If the parent runs on
        // another CPU it can reap (free_task_slot) this task before
        // schedule() switches us away, causing a use-after-free that
        // corrupts ti_cpu (initialised to -1 in Task::new).
        // Instead, store the parent PID in a per-CPU deferred slot;
        // __schedule processes it AFTER the context switch, when this
        // task is no longer running on any CPU.
        //
        // 99th (orphan-during-exit parity, §87.8 hunt): the reference's
        // do_notify_parent resolves task->real_parent AT NOTIFY TIME.
        // `parent_pid` above was captured at do_exit ENTRY — if the real
        // parent died while this task ran its exit tail, the child was
        // reparented (reparent_children_to_init) to a subreaper/init
        // AFTER the capture but BEFORE our ZOMBIE store, and the
        // reparent's own zombie-notify arm (which fires only for
        // children ALREADY ZOMBIE at move time) never fired for us. The
        // packed pid would then name a dead task: the delivery's
        // pid_hash lookup fails, no SIGCHLD, no wait_chldexit wake — and
        // if the new parent (init) is parked in wait4, the zombie
        // strands with its waiter forever. Resolve the CURRENT parent
        // here, inside the same preempt-guarded section: None = a racing
        // reaper already claimed us (it is inside release_task; the
        // notify would be noise) — skip; Some(p) = notify p (whether or
        // not it differs from the captured pid — same pid is the
        // identity case).
        let notify_parent: u32 = match cur.parent_ptr() {
            Some(pp) if Task::is_plausible_task_ptr(pp) => {
                crate::process::r#unsafe::borrow_task(pp as *mut Task).pid()
            }
            _ => 0,
        };
        if notify_parent != 0 {
            crate::sched::hotpath::defer_exit_notify(notify_parent, current_pid);
        }
        crate::interrupt::preempt::preempt_count_sub(1);
        // 125th waitid leg: reaper forensics — record + print every
        // leader ZOMBIE (exit time, decoded status, parent, SPAWN age,
        // last-syscall latch) so the REAP4/REAPW arms can time the reap
        // and ZLINGER can catch the never-reaped case. rux:schedtrace (retired debug=schedtrace)-
        // gated (debug/reaper.rs). Non-leaders never reach ZOMBIE (the
        // deferred-release tail above), so this fires per process.
        if crate::debug::reaper::on() {
            crate::debug::reaper::on_zombie(
                current_pid,
                exit_code,
                notify_parent,
                cur.comm(),
                cur.ext.last_syscall_nr.load(core::sync::atomic::Ordering::Relaxed),
                cur.ext.last_syscall_a0.load(core::sync::atomic::Ordering::Relaxed),
            );
        }
        if fp {
            crate::debug::forkprof::add(crate::debug::forkprof::P_EX_NOTIFY, fp_t_notify);
            crate::debug::forkprof::add(crate::debug::forkprof::P_EX_TOTAL, fp_t0);
            crate::debug::forkprof::trace(crate::debug::forkprof::T_NOTIFY_DONE, dying_pid);
        }

        // ===== do_task_dead: Final schedule, never returns =====
        // sc186/sc189: see the non-leader tail above — the terminal
        // deschedule re-asserts ZOMBIE/DEAD + unlink + preempt-bit clear
        // until a real context switch happens (the plain schedule() could
        // be repicked/refused and strand this CPU in the wfi loop).
        crate::sched::hotpath::schedule_dead_never_returns();

        loop {
            // SAFETY: `wfi` is a plain hint instruction with no side effects.
            crate::arch::cpu::wfi();
        }
    }
}

/// exit_group — terminate the WHOLE thread group (Linux do_group_exit).
///
/// Force SIGKILL every other member (leader included, whoever calls it),
/// then run the caller's do_exit. Called from:
/// - sys_exit_group (NR 94)
/// - fatal default signal actions (Linux do_group_exit semantics: a fatal
///   signal in ANY thread kills the process)
pub fn do_exit_group(exit_code: i32) -> ! {
    let cur = match crate::sched::hotpath::current() {
        Some(c) => c,
        None => do_exit(exit_code),
    };
    // gdm bring-up r9-r14: every daemon storm-death funnels through here
    // (fatal signals do_group_exit, plus explicit exit_group). Name the
    // caller, the code, and the pending-signal set that drove it — the
    // storm's first domino and the TERM->st=9 status overwrite both
    // decode from this one print.
    //
    // Serial-clean discipline (perf-fork lane, 2026-11): pr_err printed
    // on EVERY exit_group — one ~60-char line through the polled 16550
    // costs ~650 us under KVM, which was ~85% of the whole fork+exit
    // round trip (lmbench lat_proc,fork sat ~20x over the reference
    // column on this print alone; forkprof/ftrace attribution, ledger
    // perf-fork round 1). The storm hunts are done; the line moves to
    // pr_debug (hidden by the default loglevel, raisable for the next
    // hunt) so the exit path stays off the console.
    {
        // SAFETY: current is the raw pointer to the calling task.
        let ccomm = cur.comm_str();
        let pend = cur.pending.signal.load(core::sync::atomic::Ordering::Relaxed);
        crate::pr_debug!(
            "EXITGRP: pid={} comm='{}' code={} pending=0x{:x}",
            cur.pid(),
            ccomm,
            exit_code,
            pend
        );
    }
    // Publish the group exit status BEFORE stopping the siblings: the
    // SIGKILL below makes each member's fatal-signal path re-enter
    // do_exit_group with THIS code, so the whole group — the leader
    // included — reports the clean status instead of "killed by
    // signal 9" (reference signal_group_exit; LTP mq_notify03 exit(0)
    // from the mq callback thread).
    // SAFETY: current is the running task (matched above), non-null.
    let signal_arc = cur.signal_arc();
    if let Some(ss) = signal_arc {
        ss.group_exit_code
            .store(exit_code, core::sync::atomic::Ordering::Release);
        ss.group_exiting
            .store(true, core::sync::atomic::Ordering::Release);
    }
    crate::api::process::kill_other_threads(cur);
    do_exit(exit_code)
}

/// Move every child of `dying` to a new parent (Linux
/// forget_original_parent): the nearest PR_SET_CHILD_SUBREAPER ancestor if
/// one exists, else init (PID 1). Zombie children get a SIGCHLD to the new
/// parent so the shell's reaper loop can collect them; live children
/// simply change parent. Any child with a parent-death signal
/// (PR_SET_PDEATHSIG) registered receives it now. Runs before `dying`
/// transitions to ZOMBIE and under the process-tree lock, so no child can
/// observe a dangling parent.
///
/// `dying` is the currently running task; child pointers come from its
/// children list and are protected by PROCESS_TREE_LOCK.
pub const ERESTARTSYS: i32 = -512;

pub fn wake_group_chldexit(task: &Task) {
    use super::hotpath::PROCESS_TREE_LOCK;

    let _lock = PROCESS_TREE_LOCK.lock();
    let start = task as *const Task;
    let mut member = task as *const Task;
    let mut guard = 0u32;
    loop {
        let _ = crate::process::r#unsafe::borrow_task(member as *mut Task)
            .wait_chldexit
            .wake_up_all();
        member = crate::process::r#unsafe::borrow_task(member as *mut Task)
            .next_thread_ptr() as *const Task;
        if member == start || member.is_null() {
            break;
        }
        guard += 1;
        if guard > 65536 {
            break;
        }
    }
}

// ============================================================================
// wait4-park census (99th, §87.8 wait4/SIGCHLD wakeup-loss hunt)
// ============================================================================


// ════════════════ exec_ring ═══════════════════════════
// (was exec_ring.rs)
// The exec stack-zero ring, kept FRAMEWORK-side when process/exec.rs
// flipped to the process service crate: the riscv64 page-fault
// path (arch/riscv64/mm/exception.rs) walks STACKZERO_RING on the
// fault path — a hot framework read that cannot ride a hook row.
// The crate's exec policy WRITES it through this module (downward,
// pub statics); the fault handler reads it directly.

// FORENSIC: initial-stack zeroing ledger (see the zeroing site below).

impl ZeroEntry {
    const fn new() -> Self {
        Self {
            pid: core::sync::atomic::AtomicU32::new(0),
            base: core::sync::atomic::AtomicU64::new(0),
            len: core::sync::atomic::AtomicU64::new(0),
        }
    }
}
const ZERO_NEW: ZeroEntry = ZeroEntry::new();
pub static STACKZERO_RING: [ZeroEntry; 128] = [ZERO_NEW; 128];
pub static STACKZERO_CURSOR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);


// ════════════════ ptrace_core ═════════════════════════
// (was ptrace_core.rs)
// The ptrace STOP/WRITE core: the two ptrace entry points the arch
// trap files call BY NAME (the 1a criterion — a hot site's direct
// call must stay direct; no hook row sits in front of them). The
// rest of ptrace (the request dispatcher, PEEK/POKE readers,
// process_vm_copy) is process service policy reached through the
// syscall layer; it calls DOWN into these two for the stop/write
// primitives.

/// `task` is the current (about to stop) task.
pub fn ptrace_stop(task: *mut Task, sig: i32, info: SigInfo) {
    use super::types::TaskState;

    // task is current (INV-PROC-BORROW-1's current-task arm).
    {
        let t = crate::process::r#unsafe::borrow_task_mut(task);
        t.set_ptrace_siginfo(info);
        t.set_stop_signal(sig);
        t.stop_reported().store(false, core::sync::atomic::Ordering::Release);
        t.set_state(TaskState::new(TaskState::STOPPED));

        let tracer_pid = t.tracer_pid();
        if tracer_pid != 0 {
            // Tracer visibility. SIGCHLD alone is NOT reliable: its default
            // disposition is SIG_IGN (send_signal discards it for tracers
            // without a handler), so the waitqueue wake below is the real
            // notifier — it unblocks a tracer parked in wait4/waitid
            // exactly like the exit path's wake_group_chldexit.
            let _ = crate::signal::send_signal(tracer_pid, Signal::SIGCHLD as i32);
            if let Some(tr) = super::types::TaskRef::lookup(tracer_pid) {
                super::hotpath::wake_group_chldexit(&tr);
                crate::signal::signal_wake_up(tr.as_ptr());
            }
        }
        // Park in the stop until resumed. The old code only set
        // need_resched and relied on the (disabled) trap-exit preemption
        // check, so a traced task never actually stopped.
        crate::signal::park_stopped_task(task);
    }
}

/// The write goes through the kernel linear mapping, so it bypasses the
/// target PTE write permission (this is exactly what a debugger needs to
/// plant breakpoints in R+X text). Flushes the D-cache-coherent TLB/I-cache
/// afterwards so the tracee sees the new bytes.
pub fn write_target_word(task: *mut Task, addr: u64, value: u64) -> bool {
    use crate::arch::mm::{phys_to_virt, PhysAddr};

    if addr & 0o7 != 0 {
        return false;
    }
    let page_fault_addr = addr & !0xFFFu64;

    // task is pinned by the caller (sys_ptrace) for the call.
    {
        let mm = match crate::process::r#unsafe::borrow_task(task).address_space_arc() {
            Some(m) => m,
            None => return false,
        };

        if crate::process::r#unsafe::ptrace_translate(&mm, page_fault_addr).is_none() {
            let r = crate::api::mm::handle_mm_fault(
                &mm,
                crate::arch::mm::VirtAddr::new(page_fault_addr),
                crate::arch::mm::FaultFlags::READ,
            );
            if !matches!(r, crate::arch::mm::MmFaultResult::Handled) {
                return false;
            }
        }

        let (phys, page_len) = match crate::process::r#unsafe::ptrace_translate(&mm, addr) {
            Some(v) => v,
            None => return false,
        };
        let off = (addr & (page_len - 1)) as usize;
        if off + 8 > page_len as usize {
            return false;
        }
        // Kernel linear map of a user page of the target
        // (INV-PROC-PTRACE-MEM-1).
        crate::process::r#unsafe::ptrace_write_word(
            phys_to_virt(PhysAddr::new(phys + off as u64)).0 as usize,
            value,
        );
        // Coherent ordering: make the store visible before any fetch on
        // any hart (text writes — POKETEXT breakpoints).
        crate::process::r#unsafe::ptrace_flush_icache(addr);
        true
    }
}


// ════════════════ ns ══════════════════════════════════
// (was ns.rs)
// Namespaces (U1c) — systemd-style service isolation.
//
// Six namespace kinds following the Linux model:
// - `UtsNamespace`   — hostname / domainname (fully isolated storage)
// - `MountNamespace` — private mount table (copy-on-write Vec of entries;
//   the dentry-level VFS remains global — recorded divergence)
// - `PidNamespace`   — nested PID numbering; a task carries its ns-local
//   pid (`Task::ns_local_pid`) plus the ns pointer; kill/wait translate
// - `NetNamespace`   — stub (devices/addresses stay global)
// - `IpcNamespace`   — stub (SysV IPC objects stay global)
// - `UserNamespace`  — stub (uid/gid mapping is identity; only the initial
//   namespace is truly supported)
//
// `None` in a Task ns slot means "initial namespace" — every accessor
// falls back to the lazily created init instance, so boot-time tasks and
// kernel threads need no explicit wiring.
//
// Syscalls: unshare(2), setns(2) (here); /proc/[pid]/ns/{kind} magic
// symlinks + ns fds (open intercepted in sys_openat) live here too.

// ============================================================================
// CLONE_NEW* flag values (mirror process::fork; duplicated so this module
// has no dependency on the fork path)
// ============================================================================

pub const CLONE_NEWNS: u64 = 0x00020000;
pub const CLONE_NEWCGROUP: u64 = 0x02000000;
pub const CLONE_NEWUTS: u64 = 0x04000000;
pub const CLONE_NEWIPC: u64 = 0x08000000;
pub const CLONE_NEWUSER: u64 = 0x10000000;
pub const CLONE_NEWPID: u64 = 0x20000000;
pub const CLONE_NEWNET: u64 = 0x40000000;

/// All namespace-creating CLONE flags.
pub const CLONE_NEWMASK: u64 = CLONE_NEWNS
    | CLONE_NEWCGROUP
    | CLONE_NEWUTS
    | CLONE_NEWIPC
    | CLONE_NEWUSER
    | CLONE_NEWPID
    | CLONE_NEWNET;

// ============================================================================
// UTS namespace — hostname / domainname
// ============================================================================

impl UtsNamespace {
    fn new(inum: u64) -> Self {
        Self {
            inum,
            hostname: Spinlock::new({
                // Linux init_uts_ns defaults BOTH nodename and domainname
                // to "(none)" (kernel/uts.c) until userspace sets one. An
                // empty default made glibc's gethostname(buf, len) see a
                // zero-length name and succeed where it must fail with
                // ENAMETOOLONG (LTP gethostname02), and loggers recorded
                // an empty hostname on fresh boots.
                let mut h = [0u8; UTS_LEN];
                h[..6].copy_from_slice(b"(none)");
                h
            }),
            // Linux init_uts_ns default domainname is "(none)" — an
            // empty default makes setdomainname-restore fail EINVAL
            // (LTP setdomainname02's cleanup path warns).
            domainname: Spinlock::new({
                let mut d = [0u8; UTS_LEN];
                d[..6].copy_from_slice(b"(none)");
                d
            }),
        }
    }

    /// Write `name` into a fixed UTS buffer (NUL-padded, length-checked).
    /// A full 64-byte name is stored without NUL (Linux __NEW_UTS_LEN).
    fn uts_set(buf: &mut [u8; UTS_LEN], name: &[u8]) -> bool {
        if name.len() > UTS_LEN {
            return false;
        }
        buf[..name.len()].copy_from_slice(name);
        if name.len() < UTS_LEN {
            buf[name.len()] = 0;
        }
        true
    }

    /// Read a fixed UTS buffer as a byte string up to the NUL.
    fn uts_get(buf: &[u8; UTS_LEN]) -> Vec<u8> {
        let len = buf.iter().position(|&b| b == 0).unwrap_or(UTS_LEN);
        Vec::from(&buf[..len])
    }

    pub fn set_hostname(&self, name: &[u8]) -> bool {
        Self::uts_set(&mut self.hostname.lock(), name)
    }

    pub fn get_hostname(&self) -> Vec<u8> {
        Self::uts_get(&self.hostname.lock())
    }

    pub fn set_domainname(&self, name: &[u8]) -> bool {
        Self::uts_set(&mut self.domainname.lock(), name)
    }

    pub fn get_domainname(&self) -> Vec<u8> {
        Self::uts_get(&self.domainname.lock())
    }
}

// ============================================================================
// Mount namespace — private mount table
// ============================================================================

/// Global mount-ID allocator (Linux mnt_id_start discipline: monotonic,
/// never reused). Starts above the synthesized boot-layout ids (1-6 in
/// procfs::mounts).
static NEXT_MOUNT_ID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(16);

/// Allocate the next mount ID.
pub fn alloc_mount_id() -> u32 {
    NEXT_MOUNT_ID.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

impl Drop for MountNamespace {
    fn drop(&mut self) {
        // Last owner of this namespace is gone: detach every mount it
        // attached (Linux mntput-on-nsput). The init namespace is held
        // by a static Arc and never drops, so boot mounts are safe.
        let mounts: Vec<Arc<crate::fs::types::VfsMountInternal>> =
            core::mem::take(&mut *self.vfs_mounts.lock());
        for mnt in mounts.iter().rev() {
            crate::fs::hotpath::vfs_umount_object(mnt);
        }
    }
}

impl MountNamespace {
    fn new(inum: u64, mounts: Vec<MountEntry>) -> Self {
        Self {
            inum,
            root: Spinlock::new(crate::fs::hotpath::get_vfs_root()),
            mounts: Spinlock::new(mounts),
            vfs_mounts: Spinlock::new(Vec::new()),
        }
    }
}

/// sc98: record a freshly attached mount in the CURRENT namespace so its
/// Drop can detach it when the namespace dies (see MountNamespace::vfs_mounts).
pub fn ns_track_vfsmount(mnt: Arc<crate::fs::types::VfsMountInternal>) {
    let ns = current_mnt_ns();
    ns.vfs_mounts.lock().push(mnt);
}

/// True when the caller runs in the init (boot) mount namespace — mounts
/// made there must NOT pollute private namespaces' teardown lists (they
/// are tracked in the init ns, which never drops) and vice versa.
pub fn in_init_mnt_ns() -> bool {
    crate::sched::hotpath::current()
        .and_then(|t| t.ns_mount.clone())
        .map(|ns| Arc::ptr_eq(&ns, &init_mnt_ns()))
        .unwrap_or(true)
}

// ============================================================================
// PID namespace — nested numbering
// ============================================================================

impl PidNamespace {
    fn new(inum: u64, parent: Option<Arc<PidNamespace>>) -> Self {
        let level = parent.as_ref().map_or(0, |p| p.level + 1);
        Self {
            inum,
            parent,
            level,
            last_pid: AtomicU32::new(0),
        }
    }

    /// Allocate the next ns-local pid. Minimal: monotonic counter, wraps at
    /// pid_max. The first allocation in a fresh namespace yields 1 (the
    /// namespace init).
    pub fn alloc_local_pid(&self) -> u32 {
        loop {
            let cur = self.last_pid.load(Ordering::Relaxed);
            let next = if cur >= crate::api::process::pid_max_live() - 1 {
                1
            } else {
                cur + 1
            };
            if self
                .last_pid
                .compare_exchange_weak(cur, next, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                return next;
            }
        }
    }
}

// ============================================================================
// Stub namespaces — net / ipc / user
// ============================================================================

// ============================================================================
// Init namespaces (lazily created singletons)
// ============================================================================

/// nsfs-style inum allocator. High base avoids colliding with the procfs
/// pid-dir ino scheme (pid * 10000 + offset).
static NEXT_NS_INUM: AtomicU64 = AtomicU64::new(0xEA00_0001);

fn alloc_ns_inum() -> u64 {
    NEXT_NS_INUM.fetch_add(1, Ordering::Relaxed)
}

static INIT_UTS: Spinlock<Option<Arc<UtsNamespace>>> = Spinlock::new(None);
static INIT_MNT: Spinlock<Option<Arc<MountNamespace>>> = Spinlock::new(None);
static INIT_PID: Spinlock<Option<Arc<PidNamespace>>> = Spinlock::new(None);
static INIT_NET: Spinlock<Option<Arc<NetNamespace>>> = Spinlock::new(None);
static INIT_IPC: Spinlock<Option<Arc<IpcNamespace>>> = Spinlock::new(None);
static INIT_USER: Spinlock<Option<Arc<UserNamespace>>> = Spinlock::new(None);

/// The initial UTS namespace, seeded with hostname "rux".
pub fn init_uts_ns() -> Arc<UtsNamespace> {
    let mut guard = INIT_UTS.lock();
    if let Some(ns) = guard.as_ref() {
        return ns.clone();
    }
    let ns = Arc::new(UtsNamespace::new(alloc_ns_inum()));
    ns.set_hostname(b"rux");
    *guard = Some(ns.clone());
    ns
}

/// The initial mount namespace; its table seeds from the boot-time global
/// registry (rootfs row included).
pub fn init_mnt_ns() -> Arc<MountNamespace> {
    let mut guard = INIT_MNT.lock();
    if let Some(ns) = guard.as_ref() {
        return ns.clone();
    }
    let ns = Arc::new(MountNamespace::new(alloc_ns_inum(), boot_mount_entries()));
    *guard = Some(ns.clone());
    ns
}

pub fn init_pid_ns() -> Arc<PidNamespace> {
    let mut guard = INIT_PID.lock();
    if let Some(ns) = guard.as_ref() {
        return ns.clone();
    }
    let ns = Arc::new(PidNamespace::new(alloc_ns_inum(), None));
    *guard = Some(ns.clone());
    ns
}

pub fn init_net_ns() -> Arc<NetNamespace> {
    let mut guard = INIT_NET.lock();
    if let Some(ns) = guard.as_ref() {
        return ns.clone();
    }
    let ns = Arc::new(NetNamespace {
        inum: alloc_ns_inum(),
    });
    *guard = Some(ns.clone());
    ns
}

pub fn init_ipc_ns() -> Arc<IpcNamespace> {
    let mut guard = INIT_IPC.lock();
    if let Some(ns) = guard.as_ref() {
        return ns.clone();
    }
    let ns = Arc::new(IpcNamespace {
        inum: alloc_ns_inum(),
    });
    *guard = Some(ns.clone());
    ns
}

pub fn init_user_ns() -> Arc<UserNamespace> {
    let mut guard = INIT_USER.lock();
    if let Some(ns) = guard.as_ref() {
        return ns.clone();
    }
    let ns = Arc::new(UserNamespace {
        inum: alloc_ns_inum(),
        parent: None,
    });
    *guard = Some(ns.clone());
    ns
}

/// True when `ns` IS the init pid namespace (identity translation zone).
pub fn is_init_pid_ns(ns: &Arc<PidNamespace>) -> bool {
    Arc::ptr_eq(ns, &init_pid_ns())
}

/// The pid of `task` as seen from `ns`: its own-ns pid (chain tip),
/// an intermediate upid-chain entry, or None when the task has no pid
/// in that namespace (reference task_pid_nr_ns — an ancestor-ns task
/// is INVISIBLE in a descendant ns). The init-ns view is the global
/// pid and is handled by the callers (they see `None` slot == init).
fn upid_in_ns(task: &Task, ns: &Arc<PidNamespace>) -> Option<u32> {
    if let Some(own) = task.ns_pid.as_ref() {
        if Arc::ptr_eq(own, ns) {
            return Some(task.ns_local_pid);
        }
    }
    for (a, nr) in &task.ns_upids {
        if Arc::ptr_eq(a, ns) {
            return Some(*nr);
        }
    }
    None
}

/// Reverse of [`translate_sender_pid_ns`]: resolve a pid NUMBER written
/// by a caller living in `caller_ns` (e.g. fcntl F_SETOWN's argument) to
/// the GLOBAL pid of the task it names. None when no task matches —
/// callers map that to ESRCH-style failure.
pub fn resolve_local_pid_to_global(local: u32, caller_ns: Option<&Arc<PidNamespace>>) -> Option<u32> {
    match caller_ns {
        None => {
            let t = super::hotpath::find_task_by_pid(local)?;
            Some(t.pid())
        }
        Some(ns) => {
            let mut found = None;
            super::hotpath::pid_hash_for_each_task(|t| {
                if found.is_none() && upid_in_ns(t, ns) == Some(local) {
                    found = Some(t.pid());
                }
            });
            found
        }
    }
}

/// Translate a sender's GLOBAL pid into the pid the RECEIVER sees:
/// the sender's pid inside `receiver_ns`, or 0 when the sender has no
/// pid there (reference task_pid_nr_ns — the pid the signal's siginfo
/// must carry). Views: same ns → the own-ns pid; an ANCESTOR ns → the
/// upid-chain entry that ns assigned the sender (init ns → the global
/// pid); anything else (descendant or sibling ns) → 0 — the task is
/// invisible there.
pub fn translate_sender_pid_ns(sender_global_pid: u32, receiver_ns: Option<&Arc<PidNamespace>>) -> u32 {
    // TaskRef owns the pin (lookup_pinned/task_put paired in Drop), so
    // the sender Task stays alive across the call.
    let Some(sender) = super::types::TaskRef::lookup(sender_global_pid) else {
        return sender_global_pid;
    };
    match receiver_ns {
        // Init-ns receiver (None slot): every task's init view is
        // its global pid.
        None => sender.pid(),
        Some(r) if is_init_pid_ns(r) => sender.pid(),
        Some(r) => upid_in_ns(&sender, r).unwrap_or(0),
    }
}

// ============================================================================
// Current-task accessors (None slot == initial namespace)
// ============================================================================

pub fn current_uts_ns() -> Arc<UtsNamespace> {
    crate::sched::hotpath::current()
        .and_then(|t| t.ns_uts.clone())
        .unwrap_or_else(init_uts_ns)
}

pub fn current_mnt_ns() -> Arc<MountNamespace> {
    crate::sched::hotpath::current()
        .and_then(|t| t.ns_mount.clone())
        .unwrap_or_else(init_mnt_ns)
}

/// Pid namespace of the current task (never None).
pub fn current_pid_ns() -> Arc<PidNamespace> {
    crate::sched::hotpath::current()
        .and_then(|t| t.ns_pid.clone())
        .unwrap_or_else(init_pid_ns)
}

#[allow(dead_code)] // public API surface for upcoming net/ipc/user routing
pub fn current_net_ns() -> Arc<NetNamespace> {
    crate::sched::hotpath::current()
        .and_then(|t| t.ns_net.clone())
        .unwrap_or_else(init_net_ns)
}

#[allow(dead_code)] // public API surface for upcoming net/ipc/user routing
pub fn current_ipc_ns() -> Arc<IpcNamespace> {
    crate::sched::hotpath::current()
        .and_then(|t| t.ns_ipc.clone())
        .unwrap_or_else(init_ipc_ns)
}

#[allow(dead_code)] // public API surface for upcoming net/ipc/user routing
pub fn current_user_ns() -> Arc<UserNamespace> {
    crate::sched::hotpath::current()
        .and_then(|t| t.ns_user.clone())
        .unwrap_or_else(init_user_ns)
}

// ============================================================================
// Mount-table bookkeeping (fs::mount routes here)
// ============================================================================

/// Boot-time seed for the initial mount namespace (the hardcoded rootfs row
/// plus anything registered before the first ns access).
fn boot_mount_entries() -> Vec<MountEntry> {
    // Read the legacy global registry (populated by do_mount at boot) and
    // stamp each row with a permanent id.
    crate::fs::hotpath::get_global_mounts()
        .into_iter()
        .map(|(d, m, f, fl)| MountEntry {
            mount_id: alloc_mount_id(),
            device: d,
            mount_point: m,
            fs_type: f,
            flags: fl,
        })
        .collect()
}

/// Register a mount in the CURRENT mount namespace (fs::mount::register_mount
/// routes here). Re-mounting over an existing mountpoint replaces the row.
pub fn ns_register_mount(device: &str, mount_point: &str, fs_type: &str, flags: &str) {
    let ns = current_mnt_ns();
    let mut table = ns.mounts.lock();
    // Same-mountpoint re-registration (remount flag refresh, row
    // relocation) keeps the row's id — Linux remount keeps mnt_id.
    let existing_id = table
        .iter()
        .find(|e| e.mount_point == mount_point)
        .map(|e| e.mount_id);
    table.retain(|e| e.mount_point != mount_point);
    let mount_id = existing_id.unwrap_or_else(alloc_mount_id);
    table.push(MountEntry {
        mount_id,
        device: String::from(device),
        mount_point: String::from(mount_point),
        fs_type: String::from(fs_type),
        flags: String::from(flags),
    });
}

/// Drop a mount row from the current namespace (umount bookkeeping).
pub fn ns_unregister_mount(mount_point: &str) {
    let ns = current_mnt_ns();
    let mut table = ns.mounts.lock();
    table.retain(|e| e.mount_point != mount_point);
}

/// Snapshot of the current namespace's mount table.
pub fn ns_get_mounts() -> Vec<MountEntry> {
    current_mnt_ns().mounts.lock().clone()
}

/// True when `path` is a mountpoint of the current mount namespace.
pub fn ns_is_mountpoint(path: &str) -> bool {
    let norm = crate::fs::hotpath::path_normalize(path);
    let ns = current_mnt_ns();
    let table = ns.mounts.lock();
    table.iter().any(|e| e.mount_point == norm)
}

// ============================================================================
// PID translation (vpids)
// ============================================================================

/// Pid namespace that NEW children of `task` land in (honors a previous
/// unshare(CLONE_NEWPID) / setns(pid_ns_fd)).
pub fn task_pid_ns_for_children(task: &Task) -> Arc<PidNamespace> {
    task.ns_pid_for_children
        .clone()
        .or_else(|| task.ns_pid.clone())
        .unwrap_or_else(init_pid_ns)
}

/// Translate a caller-namespace pid into the global pid of its thread-group
/// LEADER. Identity in the init namespace. None when no live leader matches
/// (the caller's namespace cannot see that pid).
pub fn resolve_vpid(local: u32) -> Option<u32> {
    let ns = current_pid_ns();
    if is_init_pid_ns(&ns) {
        return Some(local);
    }
    let mut found: Option<u32> = None;
    // SAFETY: callback receives hash-table task pointers; we only read
    // pid/ns fields that are immutable after fork-enqueue.
    {
        super::hotpath::pid_hash_for_each_task(|task| {
            if found.is_some() {
                return;
            }
            if upid_in_ns(task, &ns) == Some(local) && task.pid() == task.tgid() {
                found = Some(task.pid());
            }
        });
    }
    found
}

/// Translate a caller-namespace TID into the global tid — the THREAD-level
/// variant of resolve_vpid (any task in the caller's namespace matches, not
/// just thread-group leaders). This is the reference's find_vpid(pid) as
/// used by do_tgkill/do_send_specific: tgkill(2) takes caller-ns numbers
/// (musl raise() passes getpid()/gettid() results, which are ns-local), so
/// resolving them globally misfires inside a pid namespace.
pub fn resolve_vtid(local: u32) -> Option<u32> {
    let ns = current_pid_ns();
    if is_init_pid_ns(&ns) {
        return Some(local);
    }
    let mut found: Option<u32> = None;
    // SAFETY: callback receives hash-table task pointers; we only read
    // pid/ns fields that are immutable after fork-enqueue.
    {
        super::hotpath::pid_hash_for_each_task(|task| {
            if found.is_some() {
                return;
            }
            if upid_in_ns(task, &ns) == Some(local) {
                found = Some(task.pid());
            }
        });
    }
    found
}

/// Global pid of a task as seen from the CURRENT task's pid namespace.
/// Same-ns and ancestor-ns views resolve through the own-ns pid / upid
/// chain; the init-ns view is the global pid; a task has NO pid in a
/// descendant or sibling ns (0), per reference task_pid_nr_ns.
#[allow(dead_code)] // public API surface for upcoming net/ipc/user routing
pub fn task_vpid(task: &Task) -> u32 {
    let ns = current_pid_ns();
    if is_init_pid_ns(&ns) {
        return task.pid();
    }
    upid_in_ns(task, &ns).unwrap_or(0)
}

/// True when two tasks share a pid namespace (both None == init ns).
pub fn task_same_pid_ns(a: &Task, b: &Task) -> bool {
    match (a.ns_pid.as_ref(), b.ns_pid.as_ref()) {
        (None, None) => true,
        (Some(x), Some(y)) => Arc::ptr_eq(x, y),
        _ => false,
    }
}

/// wait4(2) reporting: a reaped child's pid as seen from the caller's pid
/// namespace. Falls back to the global pid when the task already left the
/// hash (late translation) or lives in another namespace.
pub fn wait_vpid(global_pid: u32) -> u32 {
    let ns = current_pid_ns();
    if is_init_pid_ns(&ns) {
        return global_pid;
    }
    match super::hotpath::find_task_by_pid(global_pid) {
        Some(t) => upid_in_ns(&t, &ns).unwrap_or(global_pid),
        None => global_pid,
    }
}

// ============================================================================
// fork / unshare / setns
// ============================================================================

/// Capability gate for namespace creation (Linux copy_namespaces in
/// kernel/fork.c): only the act of CREATING a namespace is gated. A request
/// without any CLONE_NEW* bit (a plain fork/clone that shares the parent's
/// namespaces) passes unconditionally. CLONE_NEWUSER is exempt: an
/// unprivileged user may create a user namespace (the new userns then owns
/// the process, where it holds all capabilities).
///
/// The old condition inverted this: it demanded CAP_SYS_ADMIN whenever
/// CLONE_NEWUSER was ABSENT — i.e. for every plain fork — so any process
/// that had dropped privileges (setuid04 after setuid(nobody)) got EPERM
/// from fork().
fn check_ns_capability(flags: u64) -> Result<(), i32> {
    // Bits (other than CLONE_NEWUSER) that would create a new namespace.
    let gated = flags & !CLONE_NEWUSER;
    if gated == 0 {
        return Ok(());
    }
    if !crate::security::capable(crate::security::CAP_SYS_ADMIN) {
        return Err(-1); // EPERM
    }
    Ok(())
}

/// Namespace half of copy_process: the child either shares the parent's
/// namespace objects (plain Arc clone — fork default) or gets fresh ones
/// for every CLONE_NEW* bit. `child` is not yet runnable.
pub fn copy_namespaces(parent: &Task, child: &mut Task, flags: u64) -> Result<(), i32> {
    check_ns_capability(flags & CLONE_NEWMASK)?;

    // --- UTS ---
    if flags & CLONE_NEWUTS != 0 {
        let ns = Arc::new(UtsNamespace::new(alloc_ns_inum()));
        // Linux copy_utsname: the child starts with a COPY of the parent's
        // names, then diverges.
        let (hn, dn) = match parent.ns_uts.as_ref() {
            Some(p) => (p.get_hostname(), p.get_domainname()),
            None => (
                init_uts_ns().get_hostname(),
                init_uts_ns().get_domainname(),
            ),
        };
        ns.set_hostname(&hn);
        ns.set_domainname(&dn);
        child.ns_uts = Some(ns);
    } else {
        child.ns_uts = parent.ns_uts.clone();
    }

    // --- mount ---
    if flags & CLONE_NEWNS != 0 {
        let table = match parent.ns_mount.as_ref() {
            Some(p) => p.mounts.lock().clone(),
            None => init_mnt_ns().mounts.lock().clone(),
        };
        let ns = Arc::new(MountNamespace::new(alloc_ns_inum(), table));
        // Inherit the parent's pivot anchor: CLONE_NEWNS inside a pivoted
        // ns keeps the same view (the copy sees the same ns root).
        if let Some(p) = parent.ns_mount.as_ref() {
            *ns.root.lock() = p.root.lock().clone();
        }
        child.ns_mount = Some(ns);
    } else {
        child.ns_mount = parent.ns_mount.clone();
    }

    // --- pid ---
    let parent_ns = parent.ns_pid.clone().unwrap_or_else(init_pid_ns);
    if flags & CLONE_NEWPID != 0 {
        let ns = Arc::new(PidNamespace::new(alloc_ns_inum(), Some(parent_ns)));
        child.ns_pid = Some(ns.clone());
        // The namespace init: first local pid is 1.
        child.ns_local_pid = ns.alloc_local_pid();
        // Reference alloc_pid: the task also gets a number in EVERY
        // ancestor namespace. The init-ns number is the global pid
        // (root view, by construction); intermediate levels go on the
        // upid chain. Without the chain, a depth-2 task's clone(2)
        // return (creator-ns view) and its wait/kill lookups mismatched
        // — the kselftest memfd nested-sysctl child aborted with ECHILD
        // (waitpid on its own just-spawned child).
        let mut ancestor = ns.parent.clone();
        while let Some(ans) = ancestor {
            if is_init_pid_ns(&ans) {
                break;
            }
            child.ns_upids.push((ans.clone(), ans.alloc_local_pid()));
            ancestor = ans.parent.clone();
        }
    } else {
        // Children land where the parent's ns_pid_for_children points
        // (unshare(CLONE_NEWPID) / setns(pid_ns_fd) semantics).
        let target = task_pid_ns_for_children(parent);
        if is_init_pid_ns(&target) {
            child.ns_pid = None; // slot None == initial namespace
            child.ns_local_pid = child.pid(); // identity in the init ns
        } else {
            child.ns_pid = Some(target.clone());
            child.ns_local_pid = target.alloc_local_pid();
            // Same upid-chain build: the ancestors between the target
            // ns and init each assign the child a number.
            let mut ancestor = target.parent.clone();
            while let Some(ans) = ancestor {
                if is_init_pid_ns(&ans) {
                    break;
                }
                child.ns_upids.push((ans.clone(), ans.alloc_local_pid()));
                ancestor = ans.parent.clone();
            }
        }
    }

    // --- stubs ---
    if flags & CLONE_NEWNET != 0 {
        child.ns_net = Some(Arc::new(NetNamespace {
            inum: alloc_ns_inum(),
        }));
    } else {
        child.ns_net = parent.ns_net.clone();
    }
    if flags & CLONE_NEWIPC != 0 {
        child.ns_ipc = Some(Arc::new(IpcNamespace {
            inum: alloc_ns_inum(),
        }));
    } else {
        child.ns_ipc = parent.ns_ipc.clone();
    }
    if flags & CLONE_NEWUSER != 0 {
        let p = parent.ns_user.clone();
        child.ns_user = Some(Arc::new(UserNamespace {
            inum: alloc_ns_inum(),
            parent: p,
        }));
    } else {
        child.ns_user = parent.ns_user.clone();
    }

    Ok(())
}

/// unshare(2) namespace half: give the CALLING task fresh namespaces for
/// every CLONE_NEW* bit. CLONE_NEWPID only switches the children target
/// (Linux: the caller keeps its own pids).
pub fn unshare_namespaces(flags: u64) -> Result<(), i32> {
    let nsflags = flags & CLONE_NEWMASK;
    if nsflags == 0 {
        return Ok(());
    }
    check_ns_capability(nsflags)?;

    let Some(task) = crate::sched::hotpath::current() else {
        return Err(-1); // EPERM
    };

    // We are mutating the current task's own ns slots; no other CPU
    // holds a reference through them concurrently (fork takes the
    // parent's slots under the same task context).
    {
        if nsflags & CLONE_NEWUTS != 0 {
            let old = task_uts_of(task);
            let ns = Arc::new(UtsNamespace::new(alloc_ns_inum()));
            ns.set_hostname(&old.get_hostname());
            ns.set_domainname(&old.get_domainname());
            task.ns_uts = Some(ns);
        }
        if nsflags & CLONE_NEWNS != 0 {
            let table = task_mnt_table_of(task);
            let old_root = task
                .ns_mount
                .as_ref()
                .map(|p| p.root.lock().clone())
                .unwrap_or_else(|| crate::fs::hotpath::get_vfs_root());
            let ns = Arc::new(MountNamespace::new(alloc_ns_inum(), table));
            // Inherit the previous ns's pivot anchor (see the clone site).
            *ns.root.lock() = old_root;
            task.ns_mount = Some(ns);
        }
        if nsflags & CLONE_NEWPID != 0 {
            // The caller itself stays where it is; only future children are
            // created inside the new namespace.
            let cur = task
                .ns_pid
                .clone()
                .unwrap_or_else(init_pid_ns);
            task.ns_pid_for_children = Some(Arc::new(PidNamespace::new(
                alloc_ns_inum(),
                Some(cur),
            )));
        }
        if nsflags & CLONE_NEWNET != 0 {
            task.ns_net = Some(Arc::new(NetNamespace {
                inum: alloc_ns_inum(),
            }));
        }
        if nsflags & CLONE_NEWIPC != 0 {
            task.ns_ipc = Some(Arc::new(IpcNamespace {
                inum: alloc_ns_inum(),
            }));
        }
        if nsflags & CLONE_NEWUSER != 0 {
            let p = task.ns_user.clone();
            task.ns_user = Some(Arc::new(UserNamespace {
                inum: alloc_ns_inum(),
                parent: p,
            }));
        }
    }
    Ok(())
}

/// The current task's (or the init) UTS namespace.
fn task_uts_of(task: &Task) -> Arc<UtsNamespace> {
    task.ns_uts.clone().unwrap_or_else(init_uts_ns)
}

/// The current task's (or the init) mount table snapshot.
fn task_mnt_table_of(task: &Task) -> Vec<MountEntry> {
    match task.ns_mount.as_ref() {
        Some(a) => a.mounts.lock().clone(),
        None => init_mnt_ns().mounts.lock().clone(),
    }
}

// ============================================================================
// Namespace kinds + /proc/[pid]/ns symlinks + ns fds (setns targets)
// ============================================================================

impl NsKind {
    /// /proc/[pid]/ns entry name.
    pub fn name(&self) -> &'static str {
        match self {
            NsKind::Uts => "uts",
            NsKind::Mnt => "mnt",
            NsKind::Pid => "pid",
            NsKind::Net => "net",
            NsKind::Ipc => "ipc",
            NsKind::User => "user",
        }
    }

    pub fn from_name(name: &[u8]) -> Option<NsKind> {
        match name {
            b"uts" => Some(NsKind::Uts),
            b"mnt" => Some(NsKind::Mnt),
            b"pid" => Some(NsKind::Pid),
            b"net" => Some(NsKind::Net),
            b"ipc" => Some(NsKind::Ipc),
            b"user" => Some(NsKind::User),
            _ => None,
        }
    }

    /// Corresponding CLONE_NEW* bit (setns nstype check).
    pub fn clone_flag(&self) -> u64 {
        match self {
            NsKind::Uts => CLONE_NEWUTS,
            NsKind::Mnt => CLONE_NEWNS,
            NsKind::Pid => CLONE_NEWPID,
            NsKind::Net => CLONE_NEWNET,
            NsKind::Ipc => CLONE_NEWIPC,
            NsKind::User => CLONE_NEWUSER,
        }
    }

    /// All kinds (readdir order for /proc/[pid]/ns).
    pub const ALL: [NsKind; 6] = [
        NsKind::Mnt,
        NsKind::Uts,
        NsKind::Pid,
        NsKind::Net,
        NsKind::Ipc,
        NsKind::User,
    ];

    /// Stable index (procfs ino encoding for /proc/[pid]/ns/<kind>).
    pub fn index(&self) -> u64 {
        match self {
            NsKind::Mnt => 0,
            NsKind::Uts => 1,
            NsKind::Pid => 2,
            NsKind::Net => 3,
            NsKind::Ipc => 4,
            NsKind::User => 5,
        }
    }

    /// Reverse of `index`.
    pub fn from_index(i: u64) -> Option<NsKind> {
        match i {
            0 => Some(NsKind::Mnt),
            1 => Some(NsKind::Uts),
            2 => Some(NsKind::Pid),
            3 => Some(NsKind::Net),
            4 => Some(NsKind::Ipc),
            5 => Some(NsKind::User),
            _ => None,
        }
    }
}

impl NsRef {
    pub fn kind(&self) -> NsKind {
        match self {
            NsRef::Uts(_) => NsKind::Uts,
            NsRef::Mnt(_) => NsKind::Mnt,
            NsRef::Pid(_) => NsKind::Pid,
            NsRef::Net(_) => NsKind::Net,
            NsRef::Ipc(_) => NsKind::Ipc,
            NsRef::User(_) => NsKind::User,
        }
    }

    pub fn inum(&self) -> u64 {
        match self {
            NsRef::Uts(a) => a.inum,
            NsRef::Mnt(a) => a.inum,
            NsRef::Pid(a) => a.inum,
            NsRef::Net(a) => a.inum,
            NsRef::Ipc(a) => a.inum,
            NsRef::User(a) => a.inum,
        }
    }
}

/// Open-ns-fd registry: nsfs inum → handle. Entries keep the namespace
/// alive while any fd references it (bounded by distinct-ns count).
static NS_FD_TABLE: Spinlock<Vec<(u64, NsRef)>> = Spinlock::new(Vec::new());

fn ns_fd_register(handle: NsRef) {
    let inum = handle.inum();
    let mut table = NS_FD_TABLE.lock();
    if !table.iter().any(|(i, _)| *i == inum) {
        table.push((inum, handle));
    }
}

fn ns_fd_lookup(inum: u64) -> Option<NsRef> {
    let table = NS_FD_TABLE.lock();
    table
        .iter()
        .find(|(i, _)| *i == inum)
        .map(|(_, h)| h.clone())
}

/// The namespace handle of `pid`'s `kind` slot (falls back to the init
/// namespace for tasks whose slot is None).
pub fn task_ns_handle(task: &Task, kind: NsKind) -> NsRef {
    match kind {
        NsKind::Uts => NsRef::Uts(task.ns_uts.clone().unwrap_or_else(init_uts_ns)),
        NsKind::Mnt => NsRef::Mnt(task.ns_mount.clone().unwrap_or_else(init_mnt_ns)),
        NsKind::Pid => NsRef::Pid(task.ns_pid.clone().unwrap_or_else(init_pid_ns)),
        NsKind::Net => NsRef::Net(task.ns_net.clone().unwrap_or_else(init_net_ns)),
        NsKind::Ipc => NsRef::Ipc(task.ns_ipc.clone().unwrap_or_else(init_ipc_ns)),
        NsKind::User => NsRef::User(task.ns_user.clone().unwrap_or_else(init_user_ns)),
    }
}

/// readlink(2) target for /proc/[pid]/ns/<kind>: "<kind>:[<inum>]".
pub fn ns_link_target(pid: u32, kind: NsKind) -> Vec<u8> {
    let handle = match super::hotpath::find_task_by_pid(pid) {
        Some(t) => task_ns_handle(t, kind),
        None => return alloc::format!("{}:[{}]", kind.name(), fallback_inum(kind)).into_bytes(),
    };
    alloc::format!("{}:[{}]", kind.name(), handle.inum()).into_bytes()
}

/// Init-namespace inum for a dead task's ns link.
fn fallback_inum(kind: NsKind) -> u64 {
    task_ns_handle_init(kind).inum()
}

fn task_ns_handle_init(kind: NsKind) -> NsRef {
    match kind {
        NsKind::Uts => NsRef::Uts(init_uts_ns()),
        NsKind::Mnt => NsRef::Mnt(init_mnt_ns()),
        NsKind::Pid => NsRef::Pid(init_pid_ns()),
        NsKind::Net => NsRef::Net(init_net_ns()),
        NsKind::Ipc => NsRef::Ipc(init_ipc_ns()),
        NsKind::User => NsRef::User(init_user_ns()),
    }
}

/// nsfs file read payload (Linux /proc/<pid>/ns/<kind> read): "<kind>:[<inum>]\n".
fn ns_file_content(handle: &NsRef) -> Vec<u8> {
    let mut out = alloc::format!("{}:[{}]", handle.kind().name(), handle.inum()).into_bytes();
    out.push(b'\n');
    out
}

/// Open a namespace fd for (pid, kind) — the /proc/[pid]/ns/<kind> magic
/// link target. Returns an fd whose inode ino is the ns inum and whose
/// read() returns "<kind>:[<inum>]\n".
pub fn open_ns_file(pid: u32, kind: NsKind, flags: u32) -> Result<usize, i32> {
    let file = build_ns_file(pid, kind, flags)?;
    // (unsafe-slim r3) get_file_fd_install is safe (un-signed, verified).
    crate::fs::hotpath::get_file_fd_install(file)
        .ok_or(crate::errno::Errno::TooManyOpenFiles.as_neg_i32())
}

/// Build the ns descriptor File for (pid, kind) without installing an fd
/// (the generic VFS open path — file_open on a resolved ns magic link —
/// installs it into the caller's table itself).
pub fn build_ns_file(
    pid: u32,
    kind: NsKind,
    flags: u32,
) -> Result<Arc<crate::fs::types::File>, i32> {
    let handle = super::hotpath::find_task_by_pid(pid)
        .map(|t| task_ns_handle(t, kind))
        .ok_or(crate::errno::Errno::NoSuchProcess.as_neg_i32())?;

    let content = alloc::boxed::Box::new(NsFileContent {
        data: ns_file_content(&handle),
    });

    // Static FileOps; ownership of the boxed content moves to the File
    // via set_private_data and is reclaimed in ns_file_close (the
    // framework edge pair file_private_data_ref/_take_boxed,
    // INV-PROC-NSFILE-1/2).
    let file = Arc::new(crate::fs::types::File::new(crate::fs::types::FileFlags::new(flags)));
    file.set_ops(&NS_FILE_OPS);
    file.set_private_data(alloc::boxed::Box::into_raw(content) as *mut u8);

    let mut inode = crate::fs::types::Inode::new(
        handle.inum(),
        crate::fs::types::InodeMode::new(
            crate::fs::types::InodeMode::S_IFREG | 0o444,
        ),
    );
    inode.fs_id = crate::fs::hotpath::FS_ID_PROCFS;
    file.set_inode(Arc::new(inode));

    // Keep the namespace alive for the fd's lifetime.
    ns_fd_register(handle);

    Ok(file)
}

fn ns_file_read(file: &crate::fs::types::File, buf: &mut [u8]) -> isize {
    // private_data is the NsFileContent installed by open_ns_file.
    match super::r#unsafe::file_private_data_ref::<NsFileContent>(file) {
        Some(content) => {
            let offset = file.get_pos() as usize;
            let available = content.data.len().saturating_sub(offset);
            let to_read = buf.len().min(available);
            if to_read > 0 {
                buf[..to_read].copy_from_slice(&content.data[offset..offset + to_read]);
                file.set_pos((offset + to_read) as u64);
                to_read as isize
            } else {
                0
            }
        }
        None => -9, // EBADF
    }
}

fn ns_file_close(file: &crate::fs::types::File) -> i32 {
    // The exactly-once reclaim of the constructor's Box (see ns_file_read).
    drop(super::r#unsafe::file_private_data_take_boxed::<NsFileContent>(file));
    0
}

pub(crate) static NS_FILE_OPS: crate::fs::types::FileOps = crate::fs::types::FileOps::builder()
    .read(ns_file_read)
    .close(ns_file_close)
    .build();

/// D-6 shape-test accessor: expose the private table to the unit-test
/// group `fileops-caps` without widening the runtime API.
/// Intercept /proc/[pid]/ns/<kind> opens from sys_openat. Returns
/// Some(fd-or-negative-errno) when the path matched.
pub fn try_open_ns_path(path: &str, flags: u32) -> Option<i64> {
    // (perf-syscall lane) prefix gate before the parts Vec — this runs on
    // EVERY openat (called from sys_openat's shortcut chain).
    if !path.starts_with("/proc") {
        return None;
    }
    let parts: Vec<&str> = path
        .trim_end_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    if parts.len() != 4 || parts[0] != "proc" || parts[2] != "ns" {
        return None;
    }
    let pid: u32 = if parts[1] == "self" {
        super::hotpath::current_pid()
    } else {
        match parts[1].parse::<u32>() {
            Ok(p) => p,
            Err(_) => return None,
        }
    };
    let kind = NsKind::from_name(parts[3].as_bytes())?;
    Some(match open_ns_file(pid, kind, flags) {
        Ok(fd) => fd as i64,
        Err(e) => e as i64,
    })
}

// ============================================================================
// setns(2)
// ============================================================================

/// setns(fd, nstype): reassociate the calling thread with the namespace
/// referenced by an ns fd (opened from /proc/[pid]/ns/<kind>).
///
/// PID namespaces follow Linux: the change only affects future children
/// (ns_pid_for_children). Joining a user namespace is accepted but the
/// mapping stays identity (stub).
pub fn setns(fd: i32, nstype: u64) -> Result<(), i32> {
    if fd < 0 {
        return Err(crate::errno::Errno::BadFileNumber.as_neg_i32());
    }
    if !crate::security::capable(crate::security::CAP_SYS_ADMIN) {
        return Err(crate::errno::Errno::OperationNotPermitted.as_neg_i32());
    }

    // fd validated non-negative; get_file_fd returns an Arc clone.
    let file = crate::fs::hotpath::get_file_fd(fd as usize)
        .ok_or(crate::errno::Errno::BadFileNumber.as_neg_i32())?;

    // The write-once inode cell has a safe read accessor (get_inode).
    let ino = file
        .get_inode()
        .map(|i| i.ino)
        .ok_or(crate::errno::Errno::BadFileNumber.as_neg_i32())?;

    let handle = ns_fd_lookup(ino)
        .ok_or(crate::errno::Errno::InvalidArgument.as_neg_i32())?;

    // nstype check: when nonzero it must name the fd's kind.
    if nstype != 0 && nstype != handle.kind().clone_flag() {
        return Err(crate::errno::Errno::BadFileNumber.as_neg_i32());
    }

    let Some(task) = crate::sched::hotpath::current() else {
        return Err(crate::errno::Errno::OperationNotPermitted.as_neg_i32());
    };

    // current() yields &mut Task: ns slot writes need no unsafe.
    match handle {
        NsRef::Uts(ns) => task.ns_uts = Some(ns),
        NsRef::Mnt(ns) => {
            task.ns_mount = Some(ns.clone());
            reanchor_root_for_mnt_ns(task, &ns);
        }
        NsRef::Net(ns) => task.ns_net = Some(ns),
        NsRef::Ipc(ns) => task.ns_ipc = Some(ns),
        NsRef::User(ns) => task.ns_user = Some(ns),
        NsRef::Pid(ns) => {
            // Cannot renumber ourselves: only future children enter.
            task.ns_pid_for_children = Some(ns);
        }
    }
    Ok(())
}

/// setns(CLONE_NEWNS) root re-anchoring — the pivot_root counterpart.
///
/// Reference behavior: switching mount namespaces revalidates the task's
/// fs->root/cwd against the NEW namespace's mount tree; when the root is
/// not reachable there it is reset to the new ns's root. Rux has one
/// global dentry tree, so the ns "root" is the pivot anchor recorded by
/// sys_pivot_root (a dentry distinct from the global root ⇔ this ns was
/// pivoted). Two directions matter for OH beget's sandbox model:
///  - EnterDefaultNamespace: a task pivoted into the sandbox jumps back
///    to the real "/" (init does this right after `make sandbox ...`;
///    without it init stays jailed → mkdir /storage/media ENOENT →
///    StartUser(100) never runs → no launcher).
///  - EnterSandbox: a task entering the pivoted sandbox ns gets the
///    sandbox root as its "/" (every /system/bin service child).
/// A plain-chrooted (non-pivot) task keeps its jail across setns —
/// chroot is fs state, not namespace state.
fn reanchor_root_for_mnt_ns(task: &mut Task, ns: &Arc<MountNamespace>) {
    let Some(fs) = task.fs_arc() else { return };
    let global_root = crate::fs::hotpath::get_vfs_root();
    let ns_root = ns
        .root
        .lock()
        .clone()
        .or_else(|| global_root.clone());
    // Pivoted ⇔ the ns root anchor is not the global VFS root.
    let ns_is_pivoted = match (&ns_root, &global_root) {
        (Some(n), Some(g)) => !Arc::ptr_eq(n, g),
        // No global root recorded (pre-boot): treat as non-pivoted.
        (Some(_), None) => false,
        (None, _) => false,
    };

    if ns_is_pivoted {
        let pivot_root = ns_root.unwrap();
        let same_root = fs
            .root_dentry()
            .map(|rd| Arc::ptr_eq(&rd, &pivot_root))
            .unwrap_or(false);
        if !same_root {
            let path = pivot_root.build_path();
            fs.set_root_with_dentry(path.as_bytes(), Some(pivot_root));
            fs.set_root_is_pivot(true);
            // cwd outside the new root resets to it (pivot discipline).
            let cwd = task.get_cwd();
            let cwd_norm = crate::fs::hotpath::path_normalize(
                core::str::from_utf8(&cwd).unwrap_or("/"),
            );
            if cwd_norm != "/" && !cwd_norm.starts_with(path.as_str()) {
                fs.set_cwd(b"/");
            }
            crate::pr_info!(
                "setns-mnt: pid {} re-anchored into pivoted ns root '{}'",
                task.pid(),
                path
            );
        }
    } else if fs.root_is_pivot() {
        // Leaving a pivoted ns for a non-pivoted one: root back to "/".
        if let Some(g) = global_root.as_ref() {
            fs.set_root_with_dentry(b"/", Some(g.clone()));
            fs.set_root_is_pivot(false);
            crate::pr_info!(
                "setns-mnt: pid {} pivoted root reset to '/' (default ns)",
                task.pid()
            );
        }
    }
}
