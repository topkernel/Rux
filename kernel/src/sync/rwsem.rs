//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Sleepable read-write semaphore — the mmap_lock primitive
//! (reference recipe, linux-6.6 `kernel/locking/rwsem.c`).
//!
//! This is the sleepable counterpart of `sync/rwlock.rs`. The VMA lock
//! (`MmStruct::vma_manager`) was that pure spinner and mm paths held
//! it across slow work — pagefault COW under a read guard while page
//! allocation crawls, `VmaManager::add` under the write guard — so a
//! slow reader meeting a pending writer monopolized CPUs and blocked
//! mmap writers (the 52nd's §6b evidence: mm_struct.rs:987 leak-site
//! attribution, the ohc24 frozen CPU caught spinning at syscall
//! entry). The reference's mmap_lock is a `struct rw_semaphore`:
//! waiters SLEEP, writers wait uninterruptibly (never spin the CPU
//! away), and adaptive optimistic spinning exists but is bounded and
//! owner-guarded.
//!
//! SEMANTICS PORTED (reference line numbers are linux-6.6
//! kernel/locking/rwsem.c / include/linux/rwsem.h):
//!
//! - count word bit layout (rwsem.c:117-128, 64-bit shape):
//!   `[0]` writer locked, `[1]` waiters present, `[2]` lock handoff,
//!   `[8..62]` reader count (bias `1<<8`), `[63]` read-fail guard.
//! - Fast-path reader: `fetch_add(READER_BIAS)`; falls into the
//!   slowpath on WRITER|WAITERS|HANDOFF|READFAIL (rwsem.c:241-254) —
//!   a queued waiter therefore blocks NEW readers from barging (the
//!   anti-starvation core; no invented fairness scheme).
//! - Fast-path writer: CAS 0 -> WRITER_LOCKED (rwsem.c:256-266).
//! - Writer optimistic spinning (rwsem.c:819-935): only while the
//!   owner is a WRITER that is ON-CPU (owner-word spin,
//!   rwsem_spin_on_owner:746-792), need_resched aborts, and a
//!   READER-owned lock is spun on only for a time budget
//!   `(20 + readers)/2 us` capped at 25 us (rwsem_rspin_threshold)
//!   after which the NONSPINNABLE latch is set in the owner word.
//!   There is no osq MCS queue in Rux; the SleepMutex-proven
//!   bounded-rounds budget bounds the same behavior (documented
//!   divergence: budget shape, not semantics).
//! - Waiters sleep UNINTERRUPTIBLE queued FIFO on an internal wait
//!   list protected by `wait_lock` (a genuine spinlock). Readers are
//!   granted in batches (phase-fair reader wake,
//!   rwsem_mark_wake:509-522, up to MAX_READERS_WAKEUP=0x100) by
//!   having the GRANTER pre-add their reader bias to count; a writer
//!   at the head is woken to re-run the queued-acquire CAS itself
//!   (other writers may steal from it until it wins the CAS).
//! - HANDOFF protocol (rwsem.c:604-666, RWSEM_WAIT_TIMEOUT ~4 ms,
//!   rwsem.c:359): a waiter queued longer than the timeout sets the
//!   HANDOFF bit; while it is set, fast paths and steals refuse
//!   everyone else, so the lock drains to the head waiter. HANDOFF
//!   is only ever set/cleared under wait_lock (rwsem.c:108-115).
//! - up_read wakes when the last reader leaves with waiters present;
//!   up_write always wakes with waiters present (rwsem.c:1338-1378).
//!
//! # Rust/Rux divergences (behavior-preserving, documented)
//!
//! - `owner` holds a PID (bits 0..31) plus READER_OWNED / NONSPINNABLE
//!   flag bits, not a task pointer: task lifetime for the on-CPU
//!   check is managed with `pid_hash_lookup_pinned`/`task_put` (the
//!   reference relies on preempt-disable == RCU read side around the
//!   raw pointer; Rux optimistic spinning runs preemptible).
//! - The fast paths bracket the count RMW + owner-word update with a
//!   preempt-disable section (reference rwsem.c:137-139: owner and
//!   count updates share one preempt section), but a HELD lock no
//!   longer disables preemption — sections under this lock are
//!   preemptible and may nest spinlocks below it (INV-LOCK-7 keeps
//!   its position: vma lock above the PTE section). The slowpaths
//!   re-enable preemption before parking (Rux cannot schedule() with
//!   preempt_count != 0; the reference's schedule_preempt_disabled
//!   has no Rux equivalent).
//! - The sleep/wake mechanics use Rux's state-first + re-check
//!   discipline (`Task::sleep` warning): the waiter sets
//!   UNINTERRUPTIBLE BEFORE the last grant re-check under wait_lock,
//!   so a racing grant either finds the condition already true or
//!   its `wake_up_process` lands on a sleeping-state task. No
//!   WaitQueueHead is needed: the internal wait list IS the wake
//!   registry; grant (list removal for readers, acquire-CAS for
//!   writers) is the wake token.
//! - Waiters are UNINTERRUPTIBLE only (down_read/down_write; no
//!   killable/interruptible variants — mmap_lock users don't take
//!   them in the reference either, and an uninterruptible waiter
//!   cannot exit mid-wait, so entries never outlive their tasks).
//! - No RT-task branches (Rux has one fair scheduling class;
//!   `!rt_task()` reduces to true). No lockdep/lockevent counters.
//! - Degradations (SleepMutex parity): with no current task (early
//!   boot / secondary harts) the acquire degrades to bounded pure
//!   spinning instead of refusing — single-CPU boot paths still make
//!   progress. (Unlike SleepMutex there is NO preempt_count != 0
//!   degradation: taking this lock while holding a spinlock is a
//!   lock-ordering violation that must fail loudly in review, and
//!   every VMA-lock caller runs at preempt_count 0 — trap entry from
//!   user and syscall contexts.)
//!
//! # Context restrictions
//!
//! NOT usable from interrupt context (a waiter must be able to call
//! `schedule()`). `try_read`/`try_write` never sleep and are safe
//! anywhere (the fuxstuck census relies on this).

use core::cell::UnsafeCell;
use core::fmt;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicI64, AtomicUsize, Ordering};

use crate::sync::spinlock::Spinlock;

// ==================== count word (rwsem.c:117-128) ====================

/// Bit 0: the lock is owned by a writer.
const WRITER_LOCKED: i64 = 1 << 0;
/// Bit 1: the wait list is (or was very recently) non-empty.
const FLAG_WAITERS: i64 = 1 << 1;
/// Bit 2: the lock must be handed to the first waiter.
const FLAG_HANDOFF: i64 = 1 << 2;
/// Bit 63: reader-count overflow guard (never expected to fire).
const FLAG_READFAIL: i64 = 1i64 << 63;
/// Readers count in units of 1<<8.
const READER_SHIFT: u32 = 8;
const READER_BIAS: i64 = 1 << READER_SHIFT;
/// Everything from bit 8 up (reader count + READFAIL).
const READER_MASK: i64 = !(READER_BIAS - 1);
const LOCK_MASK: i64 = WRITER_LOCKED | READER_MASK;
/// A fast-path reader fails when ANY of these is set
/// (rwsem.c:127-128) — this is what stops reader barging past queued
/// waiters.
const READ_FAILED_MASK: i64 = WRITER_LOCKED | FLAG_WAITERS | FLAG_HANDOFF | FLAG_READFAIL;

// ==================== owner word (rwsem.c:63-65) ====================

const OWNER_PID_MASK: usize = 0xFFFF_FFFF;
/// Bit 32: the lock is (or was last) owned by readers.
const OWNER_READER_OWNED: usize = 1 << 32;
/// Bit 33: optimistic spinning disabled (reader-phase spin budget
/// exceeded — rwsem_set_nonspinnable, rwsem.c:228-239).
const OWNER_NONSPINNABLE: usize = 1 << 33;

/// Waiters queued longer than this may demand a handoff
/// (rwsem.c:359: `RWSEM_WAIT_TIMEOUT = DIV_ROUND_UP(HZ, 250)`, ~4 ms).
const WAIT_TIMEOUT_JIFFIES: u64 = ((crate::config::KERNEL_HZ as u64) + 249) / 250;
/// Batch limit for one reader-phase wakeup (rwsem.c:367).
const MAX_READERS_WAKEUP: usize = 0x100;

/// `time_after(a, b)` in wrapping jiffies arithmetic.
#[inline]
fn time_after(now: u64, then: u64) -> bool {
    now.wrapping_sub(then) < (1u64 << 63)
}

/// Stall tripwire (SleepMutex STALL precedent — print-only DFX, no
/// semantic effect): a waiter parked longer than this names itself in
/// a rate-limited print. Normal parking lasts microseconds-to-
/// milliseconds; 3 s means the grant is not reaching this waiter.
const STALL_WAITER_JIFFIES: u64 = (crate::config::KERNEL_HZ as u64) * 3;

/// Global rate limit for RWSEM-STALL prints (one line / 10 s — the
/// wedge class prints hundreds of lines otherwise).
fn stall_report_ok() -> bool {
    static LAST: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let now = crate::drivers::timer::get_jiffies();
    let last = LAST.load(Ordering::Relaxed);
    now.wrapping_sub(last) >= (crate::config::KERNEL_HZ as u64) * 10
        && LAST
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
}

/// One queued waiter (reference `struct rwsem_waiter`, rwsem.c:338).
struct RwsemWaiter {
    task: *mut crate::process::Task,
    is_writer: bool,
    /// Jiffies deadline after which this waiter may demand HANDOFF.
    timeout: u64,
    /// HANDOFF latch. Written by whichever try_write_lock sets the
    /// count HANDOFF bit while this waiter is first (rwsem.c:654).
    handoff_set: bool,
}

/// Owner-word classification (rwsem.c:679-684).
#[derive(Clone, Copy, PartialEq)]
enum OwnerState {
    Null,
    Writer,
    Reader,
    Nonspinnable,
}

/// Wake-type dispatch for mark_wake (rwsem.c:348-352).
#[derive(Clone, Copy, PartialEq)]
enum WakeType {
    Any,
    Readers,
    ReadOwned,
}

/// Sleepable read-write semaphore. See the module doc for the full
/// reference mapping. Context: task/syscall only for down_*; try_*
/// are context-free.
pub struct RwSemaphore {
    count: AtomicI64,
    owner: AtomicUsize,
    /// Wait list. A genuine spinlock held for instructions at a
    /// time, never across schedule() and never across a wake (all
    /// three HANDOFF mutation sites hold it — rwsem.c:108-115).
    wait_lock: Spinlock<alloc::vec::Vec<RwsemWaiter>>,
    /// Diagnostic name (stall prints / census lines).
    name: &'static str,
}

// SAFETY: same contract as WaitQueueHead/SleepMutex — the raw task
// pointers inside the wait list are only ever passed to
// sched::wake_up_process (whose own guards handle stale targets);
// every list mutation is serialized by wait_lock; a queued waiter
// cannot exit (UNINTERRUPTIBLE), so entries never dangle.
unsafe impl Send for RwSemaphore {}
unsafe impl Sync for RwSemaphore {}

impl RwSemaphore {
    pub const fn new(name: &'static str) -> Self {
        Self {
            count: AtomicI64::new(0),
            owner: AtomicUsize::new(0),
            wait_lock: Spinlock::new(alloc::vec::Vec::new()),
            name,
        }
    }

    // ==================== owner word helpers ====================

    #[inline]
    fn owner_value(&self) -> usize {
        self.owner.load(Ordering::Relaxed)
    }

    /// Writer ownership publish (rwsem.c:140-144).
    #[inline]
    fn set_owner_writer(&self, pid: u32) {
        self.owner.store(pid as usize, Ordering::Release);
    }

    /// Owner clear on write unlock (rwsem.c:146-150).
    #[inline]
    fn clear_owner(&self) {
        self.owner.store(0, Ordering::Release);
    }

    /// Reader ownership publish, preserving NONSPINNABLE
    /// (rwsem.c:170-177, `__rwsem_set_reader_owned`). The pid is
    /// advisory (spinners only look at it when the writer flag
    /// pattern says WRITER); grant-time publishes may use 0.
    #[inline]
    fn set_reader_owned(&self, pid: u32) {
        let mut v = self.owner.load(Ordering::Relaxed);
        loop {
            let new = (pid as usize) | OWNER_READER_OWNED | (v & OWNER_NONSPINNABLE);
            match self
                .owner
                .compare_exchange_weak(v, new, Ordering::Release, Ordering::Relaxed)
            {
                Ok(_) => return,
                Err(cur) => v = cur,
            }
        }
    }

    #[inline]
    fn owner_state_of(&self, v: usize) -> OwnerState {
        if v & OWNER_NONSPINNABLE != 0 {
            OwnerState::Nonspinnable
        } else if v & OWNER_READER_OWNED != 0 {
            OwnerState::Reader
        } else if v & OWNER_PID_MASK != 0 {
            OwnerState::Writer
        } else {
            OwnerState::Null
        }
    }

    /// Latch NONSPINNABLE while the lock stays reader-owned
    /// (rwsem.c:228-239).
    fn set_nonspinnable(&self) {
        let mut v = self.owner.load(Ordering::Relaxed);
        loop {
            if v & OWNER_READER_OWNED == 0 || v & OWNER_NONSPINNABLE != 0 {
                return;
            }
            match self.owner.compare_exchange_weak(
                v,
                v | OWNER_NONSPINNABLE,
                Ordering::Release,
                Ordering::Relaxed,
            ) {
                Ok(_) => return,
                Err(cur) => v = cur,
            }
        }
    }

    /// Clear NONSPINNABLE once the reader count reaches zero
    /// (rwsem.c:941-945).
    #[inline]
    fn clear_nonspinnable(&self) {
        if self.owner.load(Ordering::Relaxed) & OWNER_NONSPINNABLE != 0 {
            self.owner.fetch_and(!OWNER_NONSPINNABLE, Ordering::Release);
        }
    }

    // ==================== fast paths ====================

    /// Acquire the read lock (down_read). Sleeps on contention.
    #[track_caller]
    pub fn down_read(&self) {
        let loc = core::panic::Location::caller();
        preempt_disable_at(loc);
        // rwsem_read_trylock (rwsem.c:241-254)
        let count = self.count.fetch_add(READER_BIAS, Ordering::Acquire);
        if count & READ_FAILED_MASK == 0 {
            self.set_reader_owned(crate::process::current_pid());
            preempt_enable();
            return;
        }
        self.down_read_slowpath(count, loc);
    }

    /// Acquire the write lock (down_write). Sleeps on contention.
    #[track_caller]
    pub fn down_write(&self) {
        let loc = core::panic::Location::caller();
        preempt_disable_at(loc);
        // rwsem_write_trylock (rwsem.c:256-266)
        if self
            .count
            .compare_exchange(0, WRITER_LOCKED, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            self.set_owner_writer(crate::process::current_pid());
            preempt_enable();
            return;
        }
        self.down_write_slowpath(loc);
    }

    /// Try to acquire the read lock without sleeping
    /// (down_read_trylock, rwsem.c:1276-1295). Refuses when a writer
    /// holds, OR any waiter is queued, OR handoff is pending.
    #[inline]
    pub fn try_read(&self) -> bool {
        let mut c = self.count.load(Ordering::Acquire);
        while c & READ_FAILED_MASK == 0 {
            match self.count.compare_exchange_weak(
                c,
                c + READER_BIAS,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.set_reader_owned(crate::process::current_pid());
                    return true;
                }
                Err(cur) => c = cur,
            }
        }
        false
    }

    /// Try to acquire the write lock without sleeping
    /// (down_write_trylock — CAS 0 -> WRITER_LOCKED).
    #[inline]
    pub fn try_write(&self) -> bool {
        if self
            .count
            .compare_exchange(0, WRITER_LOCKED, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            self.set_owner_writer(crate::process::current_pid());
            true
        } else {
            false
        }
    }

    /// Release the read lock (up_read, rwsem.c:1338-1355): wake a
    /// waiter when the last reader left with waiters present.
    #[inline]
    pub fn up_read(&self) {
        preempt_disable();
        let tmp = self.count.fetch_sub(READER_BIAS, Ordering::Release);
        if tmp & (LOCK_MASK | FLAG_WAITERS) == FLAG_WAITERS {
            self.clear_nonspinnable();
            self.rwsem_wake();
        }
        preempt_enable();
    }

    /// Release the write lock (up_write, rwsem.c:1360-1378).
    #[inline]
    pub fn up_write(&self) {
        preempt_disable();
        self.clear_owner();
        let tmp = self.count.fetch_add(-WRITER_LOCKED, Ordering::Release);
        if tmp & FLAG_WAITERS != 0 {
            self.rwsem_wake();
        }
        preempt_enable();
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.count.load(Ordering::Acquire) != 0
    }

    /// rwsem_is_contended: waiters present (lock-free approximation —
    /// FLAG_WAITERS lags one release behind the list).
    #[inline]
    pub fn is_contended(&self) -> bool {
        self.count.load(Ordering::Acquire) & FLAG_WAITERS != 0
    }

    // ==================== wake dispatch ====================

    /// rwsem_wake (rwsem.c:1203-1217): wake the queue head (writer)
    /// or grant a reader batch.
    fn rwsem_wake(&self) {
        let mut wakers = alloc::vec::Vec::with_capacity(8);
        {
            let mut list = self.wait_lock.lock();
            if !list.is_empty() {
                self.mark_wake(WakeType::Any, &mut list, &mut wakers);
            }
        }
        // Wakes strictly outside wait_lock (SleepMutex discipline:
        // never hold the state spinlock across the wake path).
        for t in wakers {
            crate::sched::wake_up_process(t);
        }
    }

    /// rwsem_mark_wake (rwsem.c:411-568), ported. Caller holds
    /// wait_lock, list non-empty. Grants are reflected into `count`
    /// and the entries BEFORE any wake; wake targets are collected
    /// into `wakers` (the reference's wake_q) for the caller to wake
    /// outside the lock.
    fn mark_wake(
        &self,
        wake_type: WakeType,
        list: &mut alloc::vec::Vec<RwsemWaiter>,
        wakers: &mut alloc::vec::Vec<*mut crate::process::Task>,
    ) {
        // rwsem.c:427-441: writer at the head — mark it for wake; it
        // re-runs the acquire CAS itself, and until it runs, other
        // writers may still steal the lock.
        if list[0].is_writer {
            if wake_type == WakeType::Any {
                wakers.push(list[0].task);
            }
            return;
        }

        // Reader phase. rwsem.c:446-447: read-fail guard.
        if self.count.load(Ordering::Acquire) < 0 {
            return;
        }

        let mut adjustment: i64 = 0;
        if wake_type != WakeType::ReadOwned {
            // rwsem.c:454-484: tentatively add ONE reader bias; if a
            // writer stole the lock, demand HANDOFF once the head
            // waiter timed out, then back the tentative add out.
            adjustment = READER_BIAS;
            let oldcount = self.count.fetch_add(adjustment, Ordering::AcqRel);
            if oldcount & WRITER_LOCKED != 0 {
                let now = crate::drivers::timer::get_jiffies();
                if time_after(now, list[0].timeout) {
                    if oldcount & FLAG_HANDOFF == 0 {
                        adjustment -= FLAG_HANDOFF;
                    }
                    list[0].handoff_set = true;
                }
                self.count.fetch_add(-adjustment, Ordering::AcqRel);
                return;
            }
            // Readers now own the lock: publish the reader phase for
            // spinners (rwsem.c:477-483).
            self.owner.fetch_or(OWNER_READER_OWNED, Ordering::Release);
        }

        // Collect up to MAX_READERS_WAKEUP readers (rwsem.c:509-522)
        // — phase-fair: every queued reader is eligible regardless of
        // position; writers stay queued.
        let mut woken: i64 = 0;
        let mut grant: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
        for i in 0..list.len() {
            if woken as usize >= MAX_READERS_WAKEUP {
                break;
            }
            if !list[i].is_writer {
                grant.push(i);
                woken += 1;
            }
        }

        // Count adjustment for the granted batch (rwsem.c:524-546):
        // queued readers' biases were backed out at queue time, so
        // add woken*BIAS (minus a tentative one already applied);
        // clear WAITERS when the list empties; clear HANDOFF once a
        // reader phase is granted.
        adjustment = woken * READER_BIAS - adjustment;
        let count_now = self.count.load(Ordering::Acquire);
        if grant.len() == list.len() {
            adjustment -= FLAG_WAITERS;
            if count_now & FLAG_HANDOFF != 0 {
                adjustment -= FLAG_HANDOFF;
            }
        } else if woken > 0 && count_now & FLAG_HANDOFF != 0 {
            adjustment -= FLAG_HANDOFF;
        }
        if adjustment != 0 {
            self.count.fetch_add(adjustment, Ordering::AcqRel);
        }

        // Second pass (rwsem.c:548-567): the grant IS the removal —
        // a granted reader's bias is already counted, so waking it
        // hands it a lock it owns. Collect wake targets.
        for i in grant.iter().rev() {
            let w = list.remove(*i);
            wakers.push(w.task);
        }
    }

    /// rwsem_try_write_lock (rwsem.c:604-666), ported. Caller holds
    /// wait_lock. `index` is the caller's slot in the list. Returns
    /// true when the write lock was acquired (entry removed, owner
    /// published).
    fn try_write_lock(
        &self,
        list: &mut alloc::vec::Vec<RwsemWaiter>,
        index: usize,
    ) -> bool {
        let mut count = self.count.load(Ordering::Acquire);
        loop {
            let has_handoff = count & FLAG_HANDOFF != 0;
            if has_handoff && list[0].handoff_set && index != 0 {
                // Honor a handoff owned by the first waiter
                // (rwsem.c:616-624).
                return false;
            }

            let mut new = count;
            if count & LOCK_MASK != 0 {
                // Lock held: only a timed-out waiter proceeds, by
                // demanding HANDOFF (rwsem.c:628-638).
                if has_handoff {
                    return false;
                }
                if !time_after(crate::drivers::timer::get_jiffies(), list[index].timeout) {
                    return false;
                }
                new |= FLAG_HANDOFF;
            } else {
                // Lock free: take it (rwsem.c:639-644).
                new |= WRITER_LOCKED;
                new &= !FLAG_HANDOFF;
                if list.len() == 1 {
                    new &= !FLAG_WAITERS;
                }
            }

            match self.count.compare_exchange_weak(
                count,
                new,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    count = new;
                    break;
                }
                Err(cur) => count = cur,
            }
        }

        if count & FLAG_HANDOFF != 0 {
            // We set the handoff bit; it belongs to the FIRST waiter
            // (rwsem.c:648-657).
            list[0].handoff_set = true;
            return false;
        }

        // Acquired — remove our entry and publish ownership
        // (rwsem.c:659-665).
        list.remove(index);
        self.set_owner_writer(crate::process::current_pid());
        true
    }

    // ==================== optimistic spinning (writers) ====================

    /// rwsem_can_spin_on_owner (rwsem.c:705-730).
    fn can_spin_on_owner(&self) -> bool {
        if crate::sched::need_resched() {
            return false;
        }
        let v = self.owner_value();
        if v & OWNER_NONSPINNABLE != 0 {
            return false;
        }
        let pid = (v & OWNER_PID_MASK) as u32;
        if pid != 0 && v & OWNER_READER_OWNED == 0 {
            // Writer-owned: profitable only while the writer is on CPU.
            if !owner_pid_on_cpu(pid) {
                return false;
            }
        }
        true
    }

    /// rwsem_spin_on_owner (rwsem.c:746-792): spin while the owner
    /// word is unchanged; stop on owner change, need_resched, or the
    /// owner leaving the CPU. The pinned owner task is looked up per
    /// owner generation (Rux owner word carries a pid).
    fn spin_on_owner(&self) -> OwnerState {
        let mut v = self.owner_value();
        let mut state = self.owner_state_of(v);
        if state != OwnerState::Writer {
            return state;
        }
        let mut pinned_pid = (v & OWNER_PID_MASK) as u32;
        let mut pinned = pid_hash_lookup_pinned(pinned_pid);
        loop {
            let new = self.owner_value();
            if new != v {
                v = new;
                state = self.owner_state_of(v);
                break;
            }
            if crate::sched::need_resched() {
                state = OwnerState::Nonspinnable;
                break;
            }
            let pid = (v & OWNER_PID_MASK) as u32;
            if pid != pinned_pid {
                if !pinned.is_null() {
                    task_put(pinned);
                }
                pinned_pid = pid;
                pinned = pid_hash_lookup_pinned(pid);
            }
            if pinned.is_null()
                || !unsafe { (*pinned).ti_on_cpu.load(Ordering::Acquire) }
            {
                state = OwnerState::Nonspinnable;
                break;
            }
            core::hint::spin_loop();
        }
        if !pinned.is_null() {
            task_put(pinned);
        }
        state
    }

    /// rwsem_try_write_lock_unqueued (rwsem.c:690-703).
    fn try_write_lock_unqueued(&self) -> bool {
        let mut count = self.count.load(Ordering::Acquire);
        while count & (LOCK_MASK | FLAG_HANDOFF) == 0 {
            match self.count.compare_exchange_weak(
                count,
                count | WRITER_LOCKED,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    self.set_owner_writer(crate::process::current_pid());
                    return true;
                }
                Err(cur) => count = cur,
            }
        }
        false
    }

    /// Reader-owned spin budget in ns (rwsem_rspin_threshold,
    /// rwsem.c:806-817: `(20 + min(readers,30))/2 us`, cap 25 us).
    fn rspin_threshold_ns(&self) -> u64 {
        let count = self.count.load(Ordering::Acquire);
        let readers = ((count >> READER_SHIFT) as u64).min(30);
        (20 + readers) * 1_000 / 2
    }

    /// rwsem_optimistic_spin (rwsem.c:819-935), bounded-rounds shape
    /// (SleepMutex budget class — no osq in Rux). Returns true when
    /// the write lock was acquired.
    fn optimistic_spin(&self) -> bool {
        let mut prev_owner_state = OwnerState::Null;
        let mut loop_count: u32 = 0;
        let mut rspin_deadline: u64 = 0;

        'outer: for _ in 0..100 {
            let owner_state = self.spin_on_owner();
            if owner_state == OwnerState::Nonspinnable {
                break 'outer;
            }

            if self.try_write_lock_unqueued() {
                return true;
            }

            if owner_state == OwnerState::Reader {
                if prev_owner_state != OwnerState::Reader {
                    // Fresh reader phase: reset the budget
                    // (rwsem.c:862-867).
                    if self.owner_value() & OWNER_NONSPINNABLE != 0 {
                        break 'outer;
                    }
                    rspin_deadline = crate::drivers::timer::monotonic_ns_since_boot()
                        + self.rspin_threshold_ns();
                    loop_count = 0;
                } else {
                    // Check the time budget once every 16 iterations
                    // (rwsem.c:876-880 — avoid a clock read per spin).
                    loop_count = loop_count.wrapping_add(1);
                    if loop_count & 0xf == 0
                        && crate::drivers::timer::monotonic_ns_since_boot() > rspin_deadline
                    {
                        self.set_nonspinnable();
                        break 'outer;
                    }
                }
            }

            // need_resched check for non-writer owners (rwsem.c:914).
            if owner_state != OwnerState::Writer && crate::sched::need_resched() {
                break 'outer;
            }
            prev_owner_state = owner_state;

            for _ in 0..64 {
                core::hint::spin_loop();
                if crate::sched::need_resched() {
                    break 'outer;
                }
            }
        }
        false
    }

    // ==================== slow paths ====================

    /// rwsem_down_read_slowpath (rwsem.c:995-1101), ported. Entered
    /// with preempt disabled (down_read's section) and our
    /// READER_BIAS already in `count`.
    fn down_read_slowpath(&self, count: i64, _loc: &'static core::panic::Location<'static>) {
        let rcnt = count >> READER_SHIFT;
        let owner = self.owner_value();

        // Reader optimistic lock stealing (rwsem.c:1003-1032): a
        // constant stream of readers must not starve a sleeping
        // waiter, so do NOT steal when readers own the lock with more
        // than one reader active.
        let reader_stream = owner & OWNER_READER_OWNED != 0
            && rcnt > 1
            && count & WRITER_LOCKED == 0;

        if !reader_stream && count & (WRITER_LOCKED | FLAG_HANDOFF) == 0 {
            // Steal: keep the bias we already added.
            self.set_reader_owned(crate::process::current_pid());
            // First reader past queued waiters: hand the reader phase
            // to the queued readers too (rwsem.c:1023-1030).
            if rcnt == 1 && count & FLAG_WAITERS != 0 {
                let mut wakers = alloc::vec::Vec::with_capacity(8);
                {
                    let mut list = self.wait_lock.lock();
                    if !list.is_empty() {
                        self.mark_wake(WakeType::ReadOwned, &mut list, &mut wakers);
                    }
                }
                for t in wakers {
                    crate::sched::wake_up_process(t);
                }
            }
            preempt_enable();
            return;
        }

        // Parkable path from here. Re-enable preemption first: Rux
        // cannot schedule() with preempt_count != 0, and the true
        // caller state (not our fast-path section) decides sleep.
        preempt_enable();

        let cur = match crate::sched::current() {
            Some(t) => t as *mut crate::process::Task,
            None => {
                // Degradation (SleepMutex parity): no task context
                // (early boot / secondary hart) — cannot sleep. Back
                // our bias out and spin on try_read.
                self.count.fetch_sub(READER_BIAS, Ordering::Release);
                loop {
                    if self.try_read() {
                        return;
                    }
                    for _ in 0..64 {
                        core::hint::spin_loop();
                    }
                }
            }
        };

        let timeout = crate::drivers::timer::get_jiffies() + WAIT_TIMEOUT_JIFFIES.max(1);
        let mut enqueued = false;
        let first_park = crate::drivers::timer::get_jiffies();

        loop {
            let mut wakers = alloc::vec::Vec::with_capacity(8);
            let granted;
            {
                let mut list = self.wait_lock.lock();

                if !enqueued {
                    // rwsem.c:1041-1057: empty queue + no writer =>
                    // our bias already owns the lock — leave now.
                    if list.is_empty()
                        && self.count.load(Ordering::Acquire) & WRITER_LOCKED == 0
                    {
                        granted = true;
                    } else {
                        // Queue (rwsem.c:1058-1061): back our bias
                        // out; announce WAITERS if the list was empty.
                        if list.is_empty() {
                            self.count
                                .fetch_add(FLAG_WAITERS - READER_BIAS, Ordering::AcqRel);
                        } else {
                            self.count.fetch_sub(READER_BIAS, Ordering::AcqRel);
                        }
                        list.push(RwsemWaiter {
                            task: cur,
                            is_writer: false,
                            timeout,
                            handoff_set: false,
                        });
                        enqueued = true;
                        granted = false;
                        // rwsem_cond_wake_waiter (rwsem.c:975-990,
                        // via rwsem.c:1063).
                        self.cond_wake_waiter(&mut list, &mut wakers);
                    }
                } else {
                    // Re-looping after a wake: the grant is the
                    // removal of our entry (mark_wake second pass).
                    granted = !list.iter().any(|w| w.task == cur);
                }
            }
            for t in wakers {
                crate::sched::wake_up_process(t);
            }

            if granted {
                self.set_reader_owned(crate::process::current_pid());
                // A concurrent grant's wake may have enqueued us
                // while we never slept — undo (no-op otherwise).
                crate::sched::dequeue_task(unsafe { &*cur });
                return;
            }

            // State-first + re-check (Task::sleep discipline): set
            // UNINTERRUPTIBLE BEFORE the final grant check under
            // wait_lock, so a racing grant's wake_up_process lands on
            // a sleeping-state task.
            unsafe {
                (*cur).set_state(crate::process::task::TaskState::new(
                    crate::process::task::TaskState::UNINTERRUPTIBLE,
                ));
            }
            let granted_now = {
                let list = self.wait_lock.lock();
                !list.iter().any(|w| w.task == cur)
            };
            // Stall tripwire (see STALL_WAITER_JIFFIES): name a waiter
            // the grant is not reaching.
            if !granted_now
                && time_after(crate::drivers::timer::get_jiffies(), first_park + STALL_WAITER_JIFFIES)
                && stall_report_ok()
            {
                let (count, owner, queued) = self.dfx_state();
                crate::pr_err!(
                    "RWSEM-STALL-R {} waiter={} waited={}j count={:#x} owner={:#x} queued={} (reader parked without grant)",
                    self.name,
                    unsafe { (*cur).pid() },
                    crate::drivers::timer::get_jiffies().wrapping_sub(first_park),
                    count,
                    owner,
                    queued
                );
            }
            if granted_now {
                unsafe {
                    (*cur).set_state(crate::process::task::TaskState::new(
                        crate::process::task::TaskState::RUNNING,
                    ));
                }
                self.set_reader_owned(crate::process::current_pid());
                crate::sched::dequeue_task(unsafe { &*cur });
                return;
            }

            crate::arch::cpu::restore_irq(true);
            crate::sched::schedule();
            // Woken (or spurious) — state is RUNNING again; loop.
        }
    }

    /// rwsem_cond_wake_waiter (rwsem.c:975-990): after enqueueing a
    /// waiter, kick the queue if the lock is not writer-held. Caller
    /// holds wait_lock.
    fn cond_wake_waiter(
        &self,
        list: &mut alloc::vec::Vec<RwsemWaiter>,
        wakers: &mut alloc::vec::Vec<*mut crate::process::Task>,
    ) {
        let count = self.count.load(Ordering::Acquire);
        if count & WRITER_LOCKED != 0 {
            return;
        }
        if count & READER_MASK != 0 {
            self.mark_wake(WakeType::Readers, list, wakers);
        } else {
            self.clear_nonspinnable();
            self.mark_wake(WakeType::Any, list, wakers);
        }
    }

    /// rwsem_down_write_slowpath (rwsem.c:1106-1197), ported. Entered
    /// with preempt disabled (down_write's section).
    fn down_write_slowpath(&self, _loc: &'static core::panic::Location<'static>) {
        // Optimistic spinning first (rwsem.c:1112-1116). Runs
        // preemptible — Rux cannot schedule() with preempt disabled;
        // the need_resched guards are inside.
        preempt_enable();
        if self.can_spin_on_owner() && self.optimistic_spin() {
            return;
        }

        let cur = match crate::sched::current() {
            Some(t) => t as *mut crate::process::Task,
            None => {
                // Degradation: no task context — pure spin.
                loop {
                    if self.try_write() {
                        return;
                    }
                    for _ in 0..64 {
                        core::hint::spin_loop();
                    }
                }
            }
        };

        let timeout = crate::drivers::timer::get_jiffies() + WAIT_TIMEOUT_JIFFIES.max(1);
        let first_park = crate::drivers::timer::get_jiffies();

        loop {
            let mut wakers = alloc::vec::Vec::with_capacity(8);
            let acquired;
            {
                let mut list = self.wait_lock.lock();

                if !list.iter().any(|w| w.task == cur) {
                    // Enqueue (rwsem.c:1127-1128).
                    list.push(RwsemWaiter {
                        task: cur,
                        is_writer: true,
                        timeout,
                        handoff_set: false,
                    });
                    // rwsem.c:1131-1145: the FIRST waiter announces
                    // WAITERS; any other kicker wakes the head.
                    if list.len() == 1 {
                        self.count.fetch_or(FLAG_WAITERS, Ordering::AcqRel);
                    } else {
                        self.cond_wake_waiter(&mut list, &mut wakers);
                    }
                }

                // Queued acquire attempt (rwsem.c:1151-1155).
                let index = list.iter().position(|w| w.task == cur).unwrap();
                acquired = self.try_write_lock(&mut list, index);
            }
            for t in wakers {
                crate::sched::wake_up_process(t);
            }
            if acquired {
                // try_write_lock removed our entry and published us
                // as owner. Undo a concurrent wake's enqueue if any.
                crate::sched::dequeue_task(unsafe { &*cur });
                return;
            }

            // State-first + re-check, then sleep.
            unsafe {
                (*cur).set_state(crate::process::task::TaskState::new(
                    crate::process::task::TaskState::UNINTERRUPTIBLE,
                ));
            }
            let acquired_now = {
                let mut list = self.wait_lock.lock();
                match list.iter().position(|w| w.task == cur) {
                    Some(i) => self.try_write_lock(&mut list, i),
                    None => true, // removed => granted earlier
                }
            };
            // Stall tripwire (see STALL_WAITER_JIFFIES).
            if !acquired_now
                && time_after(crate::drivers::timer::get_jiffies(), first_park + STALL_WAITER_JIFFIES)
                && stall_report_ok()
            {
                let (count, owner, queued) = self.dfx_state();
                // Owner liveness: a writer bit with a dead owner pid is a
                // leaked section; a live owner is a genuinely slow holder.
                let owner_pid = (owner & OWNER_PID_MASK) as u32;
                let owner_alive = if owner_pid != 0 {
                    let t = pid_hash_lookup_pinned(owner_pid);
                    if t.is_null() {
                        false
                    } else {
                        let alive = !unsafe { (*t).state() }.is_dead();
                        task_put(t);
                        alive
                    }
                } else {
                    false
                };
                crate::pr_err!(
                    "RWSEM-STALL-W {} waiter={} waited={}j count={:#x} owner={} alive={} queued={} (writer parked without grant)",
                    self.name,
                    unsafe { (*cur).pid() },
                    crate::drivers::timer::get_jiffies().wrapping_sub(first_park),
                    count,
                    owner_pid,
                    owner_alive,
                    queued
                );
            }
            if acquired_now {
                unsafe {
                    (*cur).set_state(crate::process::task::TaskState::new(
                        crate::process::task::TaskState::RUNNING,
                    ));
                }
                crate::sched::dequeue_task(unsafe { &*cur });
                return;
            }

            crate::arch::cpu::restore_irq(true);
            crate::sched::schedule();
            // Woken: loop re-runs the queued acquire.
        }
    }

    /// Diagnostic snapshot (fuxstuck census / taskdump shape,
    /// try-locks only): (count, owner, queued; queued == usize::MAX
    /// means the wait lock was busy).
    pub fn dfx_state(&self) -> (i64, usize, usize) {
        let queued = match self.wait_lock.try_lock() {
            Some(l) => l.len(),
            None => usize::MAX,
        };
        (
            self.count.load(Ordering::Relaxed),
            self.owner_value(),
            queued,
        )
    }

    /// Diagnostic name.
    pub fn name(&self) -> &'static str {
        self.name
    }
}

// ==================== helpers ====================

#[inline]
fn pid_hash_lookup_pinned(pid: u32) -> *mut crate::process::Task {
    if pid == 0 {
        return core::ptr::null_mut();
    }
    crate::process::pid_hash::pid_hash_lookup_pinned(pid)
}

#[inline]
fn task_put(t: *mut crate::process::Task) {
    crate::process::task::Task::task_put(t);
}

#[inline]
fn owner_pid_on_cpu(pid: u32) -> bool {
    let t = pid_hash_lookup_pinned(pid);
    if t.is_null() {
        return false;
    }
    let on = unsafe { (*t).ti_on_cpu.load(Ordering::Acquire) };
    task_put(t);
    on
}

#[inline]
#[track_caller]
fn preempt_disable_at(loc: &'static core::panic::Location<'static>) {
    crate::interrupt::preempt::preempt_count_add_at(
        crate::interrupt::preempt::PREEMPT_OFFSET,
        loc,
    );
}

#[inline]
fn preempt_enable() {
    crate::interrupt::preempt::preempt_count_sub(crate::interrupt::preempt::PREEMPT_OFFSET);
}

#[inline]
fn preempt_disable() {
    crate::interrupt::preempt::preempt_count_add(crate::interrupt::preempt::PREEMPT_OFFSET);
}

// ==================== generic Rwsem<T> ====================

/// Data-carrying sleepable rwlock (the `RwSpinlock<T>` shape on the
/// sleepable primitive). `read()`/`write()` return RAII guards; a
/// held section is PREEMPTIBLE and the holder may sleep below it
/// (never take this lock from interrupt context; never take it while
/// holding a spinlock).
pub struct Rwsem<T: ?Sized> {
    raw: RwSemaphore,
    data: UnsafeCell<T>,
}

// SAFETY: Rwsem<T> mediates all access to the inner data: shared (&T)
// via read-locked guards, exclusive (&mut T) via write-locked guards.
unsafe impl<T: ?Sized + Send> Send for Rwsem<T> {}
unsafe impl<T: ?Sized + Send + Sync> Sync for Rwsem<T> {}

impl<T: ?Sized + fmt::Debug> fmt::Debug for Rwsem<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rwsem")
            .field("locked", &self.raw.is_locked())
            .finish_non_exhaustive()
    }
}

impl<T> Rwsem<T> {
    #[inline]
    pub const fn new(name: &'static str, data: T) -> Self {
        Self {
            raw: RwSemaphore::new(name),
            data: UnsafeCell::new(data),
        }
    }

    /// Read lock (down_read). Guard drop = up_read.
    #[inline]
    #[track_caller]
    pub fn read(&self) -> RwsemReadGuard<'_, T> {
        self.raw.down_read();
        RwsemReadGuard { lock: self }
    }

    /// Write lock (down_write). Guard drop = up_write.
    #[inline]
    #[track_caller]
    pub fn write(&self) -> RwsemWriteGuard<'_, T> {
        self.raw.down_write();
        RwsemWriteGuard { lock: self }
    }

    /// Non-sleeping read trylock.
    #[inline]
    #[track_caller]
    pub fn try_read(&self) -> Option<RwsemReadGuard<'_, T>> {
        if self.raw.try_read() {
            Some(RwsemReadGuard { lock: self })
        } else {
            None
        }
    }

    /// Non-sleeping write trylock.
    #[inline]
    #[track_caller]
    pub fn try_write(&self) -> Option<RwsemWriteGuard<'_, T>> {
        if self.raw.try_write() {
            Some(RwsemWriteGuard { lock: self })
        } else {
            None
        }
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.raw.is_locked()
    }

    #[inline]
    pub fn is_contended(&self) -> bool {
        self.raw.is_contended()
    }
}

pub struct RwsemReadGuard<'a, T: ?Sized> {
    lock: &'a Rwsem<T>,
}

// SAFETY: guard exists only while the read lock is held, preventing
// any writer from obtaining exclusive access to the data.
unsafe impl<T: ?Sized + Sync> Send for RwsemReadGuard<'_, T> {}

impl<T: ?Sized> Deref for RwsemReadGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: read lock held — no &mut access can occur concurrently.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwsemReadGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.up_read();
    }
}

pub struct RwsemWriteGuard<'a, T: ?Sized> {
    lock: &'a Rwsem<T>,
}

// SAFETY: guard exists only while the write lock is held exclusively.
unsafe impl<T: ?Sized + Send> Send for RwsemWriteGuard<'_, T> {}

impl<T: ?Sized> Deref for RwsemWriteGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: write lock held exclusively.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for RwsemWriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: write lock held exclusively.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for RwsemWriteGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.up_write();
    }
}
