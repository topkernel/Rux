//! The multi-consumer event-notification plumbing (crate-plan rule:
//! plumbing MORE THAN ONE service consumes lives in the framework).
//!
//! Moved from syscall/misc.rs (Layer-1 flip step 2, 2026-11-21): the
//! epoll wake registry (sharded, allocation-free intrusive shards,
//! pin-counted notify — the AB-BA + UAF comment stack moved verbatim),
//! the signalfd wake registry, and the poll/epoll event-constant
//! tables. The epoll INSTANCE types stay in syscall; this module is
//! generic over the wake target (EpollWakeClient) so no service type
//! crosses upward.
//!
//! INV-SYSC-1/2 contracts (the list walks + the pin discipline) moved
//! with the code; the audited list helpers are replicated from
//! syscall/unsafe.rs with the same INV-SYSC-1 wording.

use crate::list::ListHead;
use crate::sync::spinlock::Spinlock;

// ==================== poll/epoll event constants ====================

/// poll event types
pub mod poll_events {
    pub const POLLIN: u16 = 0x0001;
    pub const POLLPRI: u16 = 0x0002;
    pub const POLLOUT: u16 = 0x0004;
    pub const POLLERR: u16 = 0x0008;
    pub const POLLHUP: u16 = 0x0010;
    pub const POLLNVAL: u16 = 0x0020;
    pub const POLLRDNORM: u16 = 0x0040;
    pub const POLLRDBAND: u16 = 0x0080;
    pub const POLLWRNORM: u16 = 0x0100;
    pub const POLLWRBAND: u16 = 0x0200;
    /// POLLRDHUP (Linux _POLLRDHUP; the epoll EPOLLRDHUP bit has the same
    /// 0x2000 value). Maskable, like the other readiness bits.
    pub const POLLRDHUP: u16 = 0x2000;
}

/// epoll_event structure
// Linux uapi packs epoll_event ONLY on x86-64
// (include/uapi/linux/eventpoll.h: EPOLL_PACKED under __x86_64__); every
// other arch — riscv64 included — uses the natural C layout: events@0,
// data@8, 16-byte records. glibc and musl mirror the uapi (verified:
// Ubuntu 22.04 riscv64 glibc sizeof(epoll_event)=16, data@8; musl 1.2.5
// applies __packed__ only under __x86_64__).
//
// R72: making the struct unconditionally packed fixed the x86_64 stride
// (LTP epoll_wait01) but broke riscv64: the kernel wrote 12-byte records
// with data@4 while both riscv64 libcs read 16-byte records with data@8,
// so every `data` cookie came back as a mix of padding and neighboring
// bytes — Xorg's InputThread dereferenced a mangled epoll cookie and
// SIGSEGV'd at 0x4000b80 on every boot (deterministic, t=8s), which with
// force-unblocked synchronous signals took the whole server down. Keep
// each arch byte-identical to its uapi shape.
#[cfg_attr(feature = "x86_64", repr(C, packed))]
#[cfg_attr(not(feature = "x86_64"), repr(C))]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct EPollEvent {
    pub events: u32,
    pub data: u64,
}

/// epoll event types
pub mod epoll_events {
    pub const EPOLLIN: u32 = 0x00000001;
    pub const EPOLLPRI: u32 = 0x00000002;
    pub const EPOLLOUT: u32 = 0x00000004;
    pub const EPOLLERR: u32 = 0x00000008;
    pub const EPOLLHUP: u32 = 0x00000010;
    pub const EPOLLRDHUP: u32 = 0x00002000;
    pub const EPOLLONESHOT: u32 = 0x40000000;
    pub const EPOLLET: u32 = 1 << 31;
}

// ==================== audited list helpers (INV-SYSC-1 twins) ====================

#[inline]
fn list_first(head: *mut ListHead) -> *mut ListHead {
    debug_assert!(!head.is_null());
    // SAFETY: INV-SYSC-1 — plain load of the sentinel's next field.
    unsafe { (*head).next }
}

#[inline]
fn list_next(cur: *mut ListHead) -> *mut ListHead {
    // SAFETY: INV-SYSC-1 — capture-before-body discipline.
    unsafe { (*cur).next }
}

#[inline]
fn list_link_tail(node: *mut ListHead, head: *mut ListHead) {
    debug_assert!(!node.is_null() && !head.is_null());
    // SAFETY: INV-SYSC-1 — node's list is initialized (fresh) and
    // unlinked; head is the live sentinel held under the lock.
    unsafe { (*node).add_tail(head) }
}

#[inline]
fn list_unlink(node: *mut ListHead) {
    // SAFETY: INV-SYSC-1 unlink arm — caller holds the list's lock; the
    // node is currently linked and its neighbors are live.
    unsafe { (*node).del() }
}

#[inline]
fn boxed_mut<T>(p: *mut T) -> &'static mut T {
    // SAFETY: INV-SYSC-2 — caller holds the ownership/pin proof.
    unsafe { &mut *p }
}

#[inline]
fn boxed_drop<T>(p: *mut T) {
    // SAFETY: INV-SYSC-2 — the box is uniquely owned here (unlinked or
    // never linked).
    unsafe { drop(alloc::boxed::Box::from_raw(p)) }
}

#[inline]
fn list_entry_mut<T>(cur: *mut ListHead) -> &'static mut T {
    // SAFETY: INV-SYSC-1 — container_of to the node for a read-modify pass
    // under the owning lock.
    unsafe { &mut *(cur as *mut T) }
}

#[inline]
pub fn list_entry_pub<T>(cur: *mut ListHead) -> &'static T {
    list_entry(cur)
}

#[inline]
fn list_entry<T>(cur: *mut ListHead) -> &'static T {
    // SAFETY: INV-SYSC-1 — container_of back to the node (list is the
    // first field, so the cast is a no-op).
    unsafe { &mut *(cur as *mut T) }
}

/// The wake-target surface: the notify path's four touches of the
/// epoll instance, as trait methods so the registry stays generic and
/// no syscall type crosses into the framework.
pub trait EpollWakeClient: Sized {
    /// Pin the instance for the notify pass (the binding's presence
    /// under the shard lock proves liveness — see the pin block).
    fn notify_pin_acquire(&self);
    /// Release the pin AFTER the last touch of the instance.
    fn notify_pin_release(&self);
    /// The EPOLLET re-arm pass (epoll-instance detail).
    fn rearm_edge(&self, file_id: u64);
    /// Wake the instance's waiters.
    fn wake_waiters(&self);
}

#[inline]
fn boxed_ref<T>(p: *mut T) -> &'static T {
    // SAFETY: callers hold the pin/liveness proof (INV-SYSC-2's
    /// registry-side arm).
    unsafe { &mut *p }
}

// ==================== the epoll wake registry ====================

pub(crate) const EPOLL_WAKE_SHARDS: usize = 64;

/// One (file_id → epoll instance) binding. `list` is the FIRST field so a
/// node pointer doubles as its `ListHead` pointer (the container_of cast
/// is then a no-op) — same trick as `RcuHead`.
/// Per-node wake trampoline: performs the pinned re-arm + wake for the
/// target the registry stored (type-erased at registration — the
/// register side supplies a monomorphized fn wrapping its instance
/// type; no service type crosses into the framework).
pub type WakeFn = fn(ep: *mut u8, file_id: u64);


pub struct EpollWakeNode {
    pub list: crate::list::ListHead,
    pub file_id: u64,
    pub ep: *mut u8,
    pub wake: WakeFn,
}

/// ALLOCATION-FREE SHARD BODY (115th, allocator-under-shard-lock hunt):
/// the shard is an intrusive list of `EpollWakeNode`s — linking and
/// unlinking are two pointer writes, and the NODE is Box-allocated /
/// freed OUTSIDE the shard's irqsave section. The previous
/// `Vec<(u64, *mut T)>` body pushed (and on capacity growth,
/// ALLOCATED) inside the lock: an allocation whose per-CPU magazine row
/// was empty takes the small_alloc class lock and can walk pages / touch
/// the buddy under it — sections measured at 300 ms of real KVM time
/// under OH-boot churn (rux:allocwatch (retired debug=allocwatch), lb268). For that whole window
/// every producer mapping to the shard spins IRQs-off in
/// epoll_notify_file's lock_irqsave, no CPU on the box reports a
/// quiescent state, and the fdreap grace round prints
/// RCU-GP-BOUND site=fdreap-close — the exact print that fires
/// immediately before the OH boot deadlock wedge (DEADLOCK: spinlock
/// stuck ... EPOLL_WAKE_SHARDED + soft lockups). The sentinel cannot
/// self-reference in a const initializer, so it is lazily `init()`ed on
/// first acquisition (`init_once`, under the shard lock).
pub struct EpollWakeShard {
    pub head: crate::list::ListHead,
}

impl EpollWakeShard {
    // pub(crate) surface: the epoll diagnostics in syscall/misc walk the
    // shards read-only under the lock (same discipline as notify).
    pub(crate) fn locked_head(&mut self) -> *mut ListHead { self.init_once() }
    /// Return the sentinel pointer, self-linking it on first use. Must be
    /// called with the shard lock held.
    #[inline]
    pub fn init_once(&mut self) -> *mut crate::list::ListHead {
        if self.head.next.is_null() {
            self.head.init();
        }
        &mut self.head as *mut crate::list::ListHead
    }
}

pub static EPOLL_WAKE_SHARDED: [Spinlock<EpollWakeShard>; EPOLL_WAKE_SHARDS] =
    [const {
        crate::sync::spinlock::Spinlock::new(EpollWakeShard {
            head: crate::list::ListHead::new(),
        })
    }; EPOLL_WAKE_SHARDS];

// Live bindings per shard (leg-36 notify fast path), index-matched to
// EPOLL_WAKE_SHARDED. Moved ONLY inside the shard lock's critical
// sections (register increments, unregister_* decrement), exactly like
// WaitQueueHead::waiters: a 0 read with Acquire proves the shard has
// no bindings, so epoll_notify_file — which runs on EVERY unix-socket
// enqueue and EOF in the system — returns after one load instead of
// taking the shard's irqsave lock and scanning it. Lost-notification
// safety mirrors the waitqueue argument: a notify that reads 0 runs
// before a concurrent epoll_ctl ADD's in-lock increment publishes, and
// whether an entry being added right now catches an arrival happening
// right now is already racy in the reference kernel (the watch begins
// at ctl return). False positives (count > 0 while the shard is empty
// between an unregister and the load) just take the lock and find
// nothing, as before.
static EPOLL_SHARD_N: [core::sync::atomic::AtomicUsize; EPOLL_WAKE_SHARDS] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; EPOLL_WAKE_SHARDS];

static EPOLL_WAKE_TOTAL: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

#[inline]
fn epoll_shard_index(file_id: u64) -> usize {
    // file_id starts at 1 and increments; mask is even because
    // EPOLL_WAKE_SHARDS is a power of two.
    (file_id as usize) & (EPOLL_WAKE_SHARDS - 1)
}

#[inline]
pub fn epoll_wake_shard(file_id: u64) -> &'static crate::sync::spinlock::Spinlock<EpollWakeShard> {
    &EPOLL_WAKE_SHARDED[epoll_shard_index(file_id)]
}

/// Data arrived on (or readiness otherwise rose for) an open file
/// description: wake every epoll instance watching it and re-arm their
/// edge-triggered entries so the next epoll_wait reports the edge.
///
/// Lock order: registry → per-epoll `entries`. Every other site that
/// touches both takes them one at a time (ctl drops `entries` before
/// calling the registry helpers), so this nesting cannot deadlock.
/// (3d5f24f11 note: the notify path itself no longer nests the two — it
/// collects targets under the shard lock, releases it, then takes
/// `entries` bare.)
///
/// Safe against stale pointers: each collected target is PINNED
/// (`notify_pins`, taken under the shard lock where the binding proves
/// liveness) and `epoll_file_close` drains the pin count to zero after
/// `epoll_wake_unregister_all` and before `Box::from_raw` — a pointer
/// collected here stays alive through the entries pass and the wake even
/// if the owning epoll's last fd closes concurrently.
pub fn epoll_notify_file(file_id: u64) {
    // leg-36 lock-free empty-shard fast path: with no epoll bound in
    // this file's shard (pbench pairs, pipe-only processes, most
    // one-shot tools — and every producer before the first epoll_ctl
    // on the box), one Acquire load replaces the shard irqsave
    // lock/unlock pair and the scan below.
    if EPOLL_SHARD_N[epoll_shard_index(file_id)]
        .load(core::sync::atomic::Ordering::Acquire) == 0
    {
        // 120th input hunt: an empty shard for this file means NO epoll
        // ever bound this file id — the exact datum for the opened-but-
        // never-read face.
        if crate::trace::EVENT_INPUTTRACE.armed()
            && file_id != 0
        {
            static MISS_N: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let mn = MISS_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if mn < 24 {
                crate::pr_warn!("IN-NOTIFY-MISS #{} fid={:#x} (no epoll bound)", mn, file_id);
            }
        }
        return;
    }
    // sc103: every unix-socket enqueue runs this IRQ-save scan —
    // sharded since leg-31 (L3), so the scan is O(bindings in THIS
    // shard), not O(total registry); the leak symptom the original hunt
    // chased (dead processes' epolls growing the registry) now shows up
    // as the TOTAL counter. Name sizes BEFORE taking the lock (printk
    // discipline). 110th: the census print rides rux:epolltrace (retired debug=epolltrace) (~57
    // lines per perf boot in the spawn window).
    {
        static EPN_N: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        let n = EPN_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 400
            && (n < 40 || n % 10 == 0)
            && crate::trace::EVENT_EPOLL_DELIVERY.armed()
        {
            crate::pr_warn!(
                "epollnotify#{}: registry_total={} this_fid={:#x}",
                n,
                EPOLL_WAKE_TOTAL.load(core::sync::atomic::Ordering::Relaxed),
                file_id
            );
        }
    }
    // DEADLOCK FIX (AB-BA, 2026-11-12): the old code held the shard
    // lock while taking epoll.entries.lock and calling wake_up_all —
    // while epoll_file_close holds entries and takes ALL shard locks
    // (epoll_wake_unregister_all). Thread A (notify) held shard and
    // wanted entries; thread B (close) held entries and wanted shard.
    //
    // Fix: collect the epoll pointers under the shard lock, RELEASE
    // the shard lock, THEN touch entries and wake. Fixed-size stack
    // arrays — NO allocation under the shard lock (allocating there
    // AB-BAs with any close-path allocation that holds the allocator
    // lock and wants a shard lock).
    //
    // USE-AFTER-FREE FIX (114th audit, 2026-11-12): releasing the shard
    // lock before dereferencing the collected pointer opened a window —
    // CPU A (notify) collects `ep` and drops the shard lock; CPU B
    // (last-Arc close) runs epoll_wake_unregister_all (takes the same
    // shard lock, removes the binding) and Box::from_raw FREES the
    // instance; CPU A then locks `ep.entries` on freed (possibly
    // reallocated) memory. The notify path holds no Arc — the R20-5
    // last-reference discipline does not cover it — so the fix is a pin
    // count: take a pin on each target INSIDE the shard critical section
    // (the binding's presence proves the box is alive: the free runs
    // strictly after the unregister sweep of this very shard), and
    // epoll_file_close drains `notify_pins` to zero after unregister_all
    // before freeing. Pin hold time is one entries pass + one wake (no
    // allocation, no blocking on other locks than `entries`), so the
    // close-side drain is bounded.
    const MAX_TARGETS: usize = 32;
    // Retry bound: the batch loop below re-scans the shard until every
    // watcher of `file_id` has been processed (the old pre-shard code
    // woke ALL of them; silently stopping at 32 would swallow the ET
    // re-arm for watchers 33+ — a lost-edge wedge). 4 passes cover 128
    // distinct instances on one description, far beyond any real load;
    // beyond that, warn once and rely on the direct file-queue wakes.
    const MAX_PASSES: usize = 4;
    let mut done: [*mut u8; MAX_TARGETS * MAX_PASSES] =
        [core::ptr::null_mut(); MAX_TARGETS * MAX_PASSES];
    let mut n_done = 0usize;
    loop {
        fn no_wake(_: *mut u8, _: u64) {}
    let mut targets: [(*mut u8, WakeFn); MAX_TARGETS] =
        [(core::ptr::null_mut(), no_wake); MAX_TARGETS];
        let mut n_targets = 0usize;
        {
            let mut registry = epoll_wake_shard(file_id).lock_irqsave();
            // Intrusive-list walk (115th): read-only pass, no allocation,
            // no unlink — the scan cannot stall behind the allocator.
            // Every node reachable from the sentinel is linked under this
            // shard lock, and nodes are only unlinked under it
            // (INV-SYSC-1 via edges::list_*).
            let head = registry.init_once();
            let mut cur = list_first(head);
            while cur != head {
                let node: &EpollWakeNode = list_entry(cur);
                cur = list_next(cur);
                if node.file_id != file_id {
                    continue;
                }
                let ep = node.ep;
                if done[..n_done].contains(&ep) {
                    continue; // already woken in an earlier pass
                }
                if n_targets < MAX_TARGETS {
                    // The binding exists under this shard lock, so
                    // epoll_file_close has not freed the instance yet
                    // (its free runs after unregister_all took this
                    // lock and removed this binding). The trampoline
                    // takes/releases the pin itself (INV-SYSC-2).
                    targets[n_targets] = (ep, node.wake);
                    n_targets += 1;
                }
            }
        }
        if n_targets == 0 {
            break; // every watcher of this file_id has been processed
        }
        // Each target is pinned (the trampoline acquires inside its
        // body); the box cannot be freed until the trampoline's
        // matching release drops the last pin (INV-SYSC-2).
        for &(ep, wake_fn) in targets[..n_targets].iter() {
            // The trampoline (register-side, monomorphized over the
            // epoll instance) performs: pin acquire -> EPOLLET re-arm
            // -> wake -> pin release. All the epoll-instance detail
            // stays on the service side; the registry only calls.
            wake_fn(ep, file_id);
            if n_done < done.len() {
                done[n_done] = ep;
                n_done += 1;
            }
        }
        if n_done >= done.len() {
            static MANY_N: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let n = MANY_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < 8 {
                crate::pr_warn!(
                    "epollnotify: {} watchers of fid={:#x} exceed the retry bound; excess rely on file-queue wakes",
                    n_done,
                    file_id
                );
            }
            break;
        }
    }
}

/// Bind (file_id → epoll instance). Idempotent per pair.
///
/// ALLOCATION DISCIPLINE (115th): the node is Box-allocated BEFORE the
/// shard lock is taken and (on the idempotent re-add path) freed AFTER it
/// is released — nothing inside the irqsave section can touch the
/// allocator. The previous Vec::push allocated on capacity growth under
/// the lock; when the magazine row was empty that put the whole
/// class-lock refill (page walk, buddy) inside an IRQ-off section.
pub fn epoll_wake_register(file_id: u64, ep: *mut u8, wake: WakeFn) {
    let shard = epoll_shard_index(file_id);
    // Node is uniquely owned here; linked only under the shard lock
    // below (or freed on the idempotent path, still uniquely owned) —
    // INV-SYSC-2.
    let node = alloc::boxed::Box::into_raw(alloc::boxed::Box::new(EpollWakeNode {
        list: crate::list::ListHead::new(),
        file_id,
        ep,
        wake,
    }));
    let mut linked = false;
    {
        let mut registry = EPOLL_WAKE_SHARDED[shard].lock_irqsave();
        let head = registry.init_once();
        let mut exists = false;
        // Read-only walk of linked nodes under the shard lock
        // (INV-SYSC-1 via edges::list_*).
        let mut cur = list_first(head);
        while cur != head {
            let n: &EpollWakeNode = list_entry(cur);
            if n.file_id == file_id && n.ep == ep {
                exists = true;
                break;
            }
            cur = list_next(cur);
        }
        if !exists {
            // Node's list is initialized (fresh) and unlinked; head is
            // this shard's sentinel, held under the lock (INV-SYSC-1).
            list_link_tail(&mut boxed_mut(node).list, head);
            EPOLL_WAKE_TOTAL.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            // In-section publish (see EPOLL_SHARD_N): before the lock
            // release, so a concurrent notify's fast-path load pairs with
            // this increment.
            EPOLL_SHARD_N[shard].fetch_add(1, core::sync::atomic::Ordering::Release);
            linked = true;
        }
    }
    if !linked {
        // Idempotent re-add: reclaim the spare node OUTSIDE the lock.
        // Never linked; still uniquely owned (INV-SYSC-2).
        boxed_drop(node);
    }
}

/// Drop every binding for `ep` (epoll instance teardown). An instance may
/// watch many file_ids across shards, so sweep them all (cold path: runs
/// once per epoll close).
///
/// Unlinked nodes are freed OUTSIDE the shard sections (batched): the free
/// can hit the allocator's slow path, which must never run under one of
/// these irqsave locks (see epoll_wake_register).
pub fn epoll_wake_unregister_all(ep: *mut u8) {
    const BATCH: usize = 32;
    let mut batch: [*mut EpollWakeNode; BATCH] = [core::ptr::null_mut(); BATCH];
    for (shard, registry) in EPOLL_WAKE_SHARDED.iter().enumerate() {
        loop {
            let mut n_batch = 0usize;
            let mut hit_end;
            {
                let mut registry = registry.lock_irqsave();
                let head = registry.init_once();
                // Walk under the shard lock; unlinking captures `next`
                // before `del`, and unlinked nodes are unreachable to
                // every other CPU once the unlink lands under this lock
                // (INV-SYSC-1 via edges::list_*).
                let mut cur = list_first(head);
                hit_end = true;
                while cur != head {
                    let next = list_next(cur);
                    let n: &mut EpollWakeNode = list_entry_mut(cur);
                    if n.ep == ep && n_batch < BATCH {
                        // n.list is linked in this shard's list
                        // (INV-SYSC-1 unlink arm).
                        list_unlink(&mut n.list);
                        batch[n_batch] = cur as *mut EpollWakeNode;
                        n_batch += 1;
                        EPOLL_WAKE_TOTAL.fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
                        EPOLL_SHARD_N[shard].fetch_sub(1, core::sync::atomic::Ordering::Release);
                    } else if n.ep == ep && n_batch >= BATCH {
                        // More matches than the batch holds: stop at the
                        // sentinel and re-lock for another pass.
                        hit_end = false;
                        break;
                    }
                    cur = next;
                }
            }
            // Free the unlinked nodes with NO shard lock held. Each was
            // unlinked under the shard lock above; nodes are reachable
            // only through the shard lists (INV-SYSC-2 reclaim).
            for i in 0..n_batch {
                boxed_drop(batch[i]);
                batch[i] = core::ptr::null_mut();
            }
            if hit_end {
                break; // this shard has no bindings left for `ep`
            }
        }
    }
}

/// Drop bindings for (file_id, ep) (epoll_ctl DEL). At most one node
/// matches (register dedups per pair).
pub fn epoll_wake_unregister(file_id: u64, ep: *mut u8) {
    let shard = epoll_shard_index(file_id);
    let mut dead: core::option::Option<*mut EpollWakeNode> = None;
    {
        let mut registry = EPOLL_WAKE_SHARDED[shard].lock_irqsave();
        let head = registry.init_once();
        // Walk under the shard lock; the matching node is unlinked and
        // freed after the lock is released (INV-SYSC-1 via edges::*).
        let mut cur = list_first(head);
        while cur != head {
            let next = list_next(cur);
            let n: &mut EpollWakeNode = list_entry_mut(cur);
            if n.file_id == file_id && n.ep == ep {
                // n.list is linked in this shard's list (INV-SYSC-1).
                list_unlink(&mut n.list);
                dead = Some(cur as *mut EpollWakeNode);
                EPOLL_WAKE_TOTAL.fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
                EPOLL_SHARD_N[shard].fetch_sub(1, core::sync::atomic::Ordering::Release);
                break;
            }
            cur = next;
        }
    }
    if let Some(n) = dead {
        // Unlinked above; reachable only through the shard list
        // (INV-SYSC-2 reclaim).
        boxed_drop(n);
    }
}

// ==================== signalfd wake registry ====================

/// Owning-task → signalfd wait queue registry for signal-arrival wakeups.
/// Task pointers are compared for identity only (never dereferenced), so a
/// recycled task address causes at worst a spurious wake (readers re-check
/// readiness); entries are removed in the fd's close op.
///
/// Wrapper so the raw-pointer registry is `Send` (access under the lock).
pub struct SignalfdWakeRegistry {
    pub bindings: alloc::vec::Vec<
        (*mut crate::process::task::Task, *const crate::process::wait::WaitQueueHead),
    >,
}

static SIGNALFD_WAKE_REGISTRY: crate::sync::spinlock::Spinlock<SignalfdWakeRegistry> =
    crate::sync::spinlock::Spinlock::new(SignalfdWakeRegistry {
        bindings: alloc::vec::Vec::new(),
    });

/// Wake every signalfd whose reading task is `task` (send_signal hook).
/// Called from `signal::send_signal_locked_info` after the signal is added
/// to the target's pending set, so a blocked `read(sfd)` returns promptly
/// instead of waiting out the poll interval.
pub fn signalfd_notify_task(task: *mut crate::process::task::Task) {
    let registry = SIGNALFD_WAKE_REGISTRY.lock_irqsave();
    // SFDDBG (launcher bring-up): did the wake reach a pid-1-owned fd?
    // Zero bindings means the fd was never registered (or was closed).
    // Serial-cleanliness: opt-in via rux:sfddbg (retired debug=sfddbg).
    if crate::process::r#unsafe::task_pid_of(task) == 1
        && crate::trace::EVENT_SFDDBG.armed()
    {
        static NT_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        if NT_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 100 {
            crate::pr_info!(
                "SFDDBG: notify pid1 registry_len={}",
                registry.bindings.len()
            );
        }
    }
    for (owner, wq) in registry.bindings.iter() {
        if *owner == task {
            // wq points into a boxed SignalFd alive until its close op
            // unregisters it (close runs only on the last Arc ref) —
            // INV-SYSC-2 reachability arm.
            boxed_ref(*wq as *mut crate::process::wait::WaitQueueHead).wake_up_all();
        }
    }
}

pub fn signalfd_register(
    task: *mut crate::process::task::Task,
    wq: *const crate::process::wait::WaitQueueHead,
) {
    let mut registry = SIGNALFD_WAKE_REGISTRY.lock_irqsave();
    if !registry.bindings.iter().any(|(t, q)| *t == task && *q == wq) {
        registry.bindings.push((task, wq));
    }
}

pub fn signalfd_unregister(wq: *const crate::process::wait::WaitQueueHead) {
    let mut registry = SIGNALFD_WAKE_REGISTRY.lock_irqsave();
    registry.bindings.retain(|(_, q)| *q != wq);
}

// Send proofs for the raw-pointer registry types (moved from
// syscall/unsafe.rs with the types; same contracts: access only under
// the shard/registry locks).
unsafe impl Send for EpollWakeShard {}
unsafe impl Send for SignalfdWakeRegistry {}
