//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Miscellaneous system calls
//!
//! Includes: poll, select, pselect6, epoll_create, epoll_create1, epoll_ctl, epoll_wait,
//! epoll_pwait, eventfd, eventfd2, getrandom, read_input_event

use super::*;
use core::sync::atomic::{AtomicU32, Ordering};

/// pollfd structure (struct pollfd)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct PollFd {
    pub fd: i32,
    pub events: u16,
    pub revents: u16,
}

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

/// epoll operation types
pub mod epoll_ctl_ops {
    pub const EPOLL_CTL_ADD: i32 = 1;
    pub const EPOLL_CTL_DEL: i32 = 2;
    pub const EPOLL_CTL_MOD: i32 = 3;
}

// Global epoll instance counter (simplified implementation)
static EPOLL_INSTANCE_COUNTER: AtomicU32 = AtomicU32::new(1);

/// epoll_wait re-check interval (P1 busy-yield elimination): waiters sleep
/// on the instance's wait queue and are re-woken at least every 10ms to
/// re-poll readiness (one jiffy at the default HZ=100).
const EPOLL_RECHECK_JIFFIES: u64 = {
    let j = crate::drivers::timer::msecs_to_jiffies(10);
    if j == 0 { 1 } else { j }
};

/// Epoll monitored fd entry. `list` is the FIRST field so a node pointer
/// doubles as its `ListHead` pointer (the container_of cast is then a
/// no-op) — same intrusive-list trick as `EpollWakeNode`.
///
/// ALLOCATION-FREE LIST BODY (117th, allocator-under-entries-lock hunt):
/// the per-epoll interest set is an intrusive list of `EpollEntry`s —
/// linking and unlinking are two pointer writes, and the node is
/// Box-allocated / freed OUTSIDE the `entries` lock section. The previous
/// `Vec<EpollEntry>` body pushed (and on capacity growth, ALLOCATED)
/// inside the lock: an allocation whose per-CPU magazine row was empty
/// takes the small_alloc class lock and can walk pages / touch the buddy
/// under it — sections measured at 300 ms of real KVM time under OH-boot
/// churn (dfx=allocwatch, lb268, same evidence class as the shard-registry
/// fix). For that whole window every other thread touching that epoll's
/// entries spins; that stall was the terminal wedge of the failing OH
/// boots (CPU frozen at pc=0x103 spinning on an EpollFile.entries lock).
#[repr(C)]
struct EpollEntry {
    list: crate::list::ListHead,
    fd: i32,
    /// Opaque identity of the open file description captured at ADD time
    /// (R32-B9, R36-B1): the File's monotonic generation id. Compared for
    /// equality only, never dereferenced, so it is harmless after the file
    /// is freed — and, unlike the earlier Arc-address scheme, it can never
    /// alias a NEW file even when the allocator recycles the old address.
    /// Guards the wait path against the fd NUMBER being closed and reused
    /// by an unrelated open file.
    file_id: u64,
    events: u32,
    data: u64,
    /// EPOLLET state (P2): the full readiness set reported by the last
    /// epoll_wait that included this fd. An edge-triggered registration
    /// only reports when the current readiness set DIFFERS from this
    /// snapshot (a new edge), and readiness going fully idle resets it
    /// so the next arrival reports again. 0 = nothing reported yet.
    last_reported: u32,
    /// EPOLLONESHOT state: once an oneshot registration has reported, it
    /// stays silent — even for EPOLLERR/EPOLLHUP — until EPOLL_CTL_MOD
    /// re-arms it (epoll_ctl(2): the target is "disabled ... and the
    /// user needs to rearm it ... with EPOLL_CTL_MOD"). Without the flag
    /// honored, a second arrival re-reported a fired oneshot watch
    /// (LTP epoll_wait07: "Write channel for the 2nd time. No events
    /// expected").
    oneshot_fired: bool,
    /// dfx=socksnap (b16 appspawn hunt): how many times a wait delivered
    /// this entry — a readable LT entry with reports==0 while its epoll's
    /// wait_count grows is the lost-delivery signature.
    reports: u64,
}

/// Sentinel-headed intrusive list of `EpollEntry` nodes (see the
/// `EpollEntry` doc for the allocation discipline). The sentinel cannot
/// self-reference in a const initializer, so it is lazily `init()`ed on
/// first acquisition (`init_once`, under the `entries` lock) — same
/// pattern as `EpollWakeShard`.
struct EpollEntryList {
    head: crate::list::ListHead,
}

impl EpollEntryList {
    /// Return the sentinel pointer, self-linking it on first use. Must be
    /// called with the `entries` lock held.
    #[inline]
    fn init_once(&mut self) -> *mut crate::list::ListHead {
        if self.head.next.is_null() {
            self.head.init();
        }
        &mut self.head as *mut crate::list::ListHead
    }
}

/// Epoll file structure (stored as File private_data)
struct EpollFile {
    entries: crate::sync::spinlock::Spinlock<EpollEntryList>,
    /// Waiters blocked in epoll_wait (P1: busy-yield elimination). Woken by
    /// the re-check timer, signal delivery, or `epoll_notify_file` once
    /// fd-side producers (pipe/socket/...) are wired to call it.
    wait_queue: crate::process::wait::WaitQueueHead,
    /// dfx=socksnap (b16): creator pid, printed by the interest dump so an
    /// epoll line can be tied to its owning process.
    owner_pid: u32,
    /// dfx=socksnap (b16): total completed wait syscalls on this instance.
    wait_count: u64,
    /// In-flight `epoll_notify_file` sections holding a raw pointer to this
    /// instance (UAF close of the 3d5f24f11 window — see
    /// `epoll_notify_file`). Incremented ONLY inside a wake-shard critical
    /// section in which a binding for this instance still exists (that
    /// proves `epoll_file_close` has not yet freed the box: the free runs
    /// strictly after `epoll_wake_unregister_all` removed the binding under
    /// the same shard lock). `epoll_file_close` drains this to zero before
    /// `Box::from_raw`. fetch_sub is Release / the drain load is Acquire so
    /// the notify's entries/wait_queue work happens-before the free.
    notify_pins: core::sync::atomic::AtomicUsize,
}

// ============================================================================
// Epoll wake registry (groundwork for fd-side callback wakeups)
// ============================================================================

/// Monitored-file identity → epoll instances watching it.
///
/// `epoll_ctl(ADD)` binds (file_id, *mut EpollFile); fd-side data-arrival
/// points (unix socket enqueue, peer EOF, ...) call
/// `epoll_notify_file(file.file_id)` to (a) re-arm edge-triggered
/// registrations — resetting their readiness snapshot so the next
/// epoll_wait reports the arrival — and (b) wake any blocked waiters.
/// This is the load-bearing wake path for edge-triggered users: Xorg's
/// client sockets are EPOLLET, and a snapshot-only model silently swallows
/// an arrival that lands between a drain and the next wait (the write
/// edge happened while the entry still said "already reported").
/// Wrapper so the raw-pointer registry is `Send` (entries are only touched
/// under the registry lock; epoll pointers are unregistered in the owning
/// EpollFile's close op before the box is freed).
struct EpollWakeRegistry {
    bindings: alloc::vec::Vec<(u64, *mut EpollFile)>,
}
// SAFETY: pointers are only dereferenced under the lock and are removed
// in the owning epoll's close op, which runs before the box is freed.
unsafe impl Send for EpollWakeRegistry {}

// ============================================================================
// EPOLL_WAKE_REGISTRY sharding (leg-31, 8.10 lever L3)
// ============================================================================
// sc103 measured every unix-socket enqueue paying O(total bindings) under
// ONE global irqsave lock; with many live epolls (or leaked bindings from
// dead processes) that single lock serializes every fd-side producer on
// the box. Shard the registry by file_id: notify (the hot path — every
// socket enqueue/EOF) touches exactly ONE shard; ctl/dump touch one shard
// per (file_id) except instance teardown (unregister_all), which must
// sweep every shard (an epoll may bind many file_ids) — teardown is cold.
// file_id is a sequential generation counter (fs/file.rs), so masking the
// low bits spreads consecutive fds across shards evenly.
// Lock order is unchanged per shard: shard lock -> per-epoll `entries`
// (the registry->entries nesting note below still holds, per shard).
// EPOLL_WAKE_TOTAL keeps the sc103 diagnostic meaningful (a whole-registry
// size) without taking every shard lock to count.

const EPOLL_WAKE_SHARDS: usize = 64;

/// One (file_id → epoll instance) binding. `list` is the FIRST field so a
/// node pointer doubles as its `ListHead` pointer (the container_of cast
/// is then a no-op) — same trick as `RcuHead`.
#[repr(C)]
struct EpollWakeNode {
    list: crate::list::ListHead,
    file_id: u64,
    ep: *mut EpollFile,
}
// SAFETY: same contract as EpollWakeRegistry: `ep` is only dereferenced
// while the binding is linked (shard lock held or pinned per notify_pins),
// and the node is unlinked before the epoll's close op frees the box.
unsafe impl Send for EpollWakeNode {}

/// ALLOCATION-FREE SHARD BODY (115th, allocator-under-shard-lock hunt):
/// the shard is an intrusive list of `EpollWakeNode`s — linking and
/// unlinking are two pointer writes, and the NODE is Box-allocated /
/// freed OUTSIDE the shard's irqsave section. The previous
/// `Vec<(u64, *mut EpollFile)>` body pushed (and on capacity growth,
/// ALLOCATED) inside the lock: an allocation whose per-CPU magazine row
/// was empty takes the small_alloc class lock and can walk pages / touch
/// the buddy under it — sections measured at 300 ms of real KVM time
/// under OH-boot churn (dfx=allocwatch, lb268). For that whole window
/// every producer mapping to the shard spins IRQs-off in
/// epoll_notify_file's lock_irqsave, no CPU on the box reports a
/// quiescent state, and the fdreap grace round prints
/// RCU-GP-BOUND site=fdreap-close — the exact print that fires
/// immediately before the OH boot deadlock wedge (DEADLOCK: spinlock
/// stuck ... EPOLL_WAKE_SHARDED + soft lockups). The sentinel cannot
/// self-reference in a const initializer, so it is lazily `init()`ed on
/// first acquisition (`init_once`, under the shard lock).
struct EpollWakeShard {
    head: crate::list::ListHead,
}
// SAFETY: same contract as EpollWakeRegistry (now the per-shard type).
unsafe impl Send for EpollWakeShard {}

impl EpollWakeShard {
    /// Return the sentinel pointer, self-linking it on first use. Must be
    /// called with the shard lock held.
    #[inline]
    fn init_once(&mut self) -> *mut crate::list::ListHead {
        if self.head.next.is_null() {
            self.head.init();
        }
        &mut self.head as *mut crate::list::ListHead
    }
}

static EPOLL_WAKE_SHARDED: [crate::sync::spinlock::Spinlock<EpollWakeShard>; EPOLL_WAKE_SHARDS] =
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
fn epoll_wake_shard(file_id: u64) -> &'static crate::sync::spinlock::Spinlock<EpollWakeShard> {
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
        if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::InputTrace)
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
    // discipline). 110th: the census print rides dfx=epolltrace (~57
    // lines per perf boot in the spawn window).
    {
        static EPN_N: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        let n = EPN_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 400
            && (n < 40 || n % 10 == 0)
            && crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::EpollTrace)
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
    let mut done: [*mut EpollFile; MAX_TARGETS * MAX_PASSES] =
        [core::ptr::null_mut(); MAX_TARGETS * MAX_PASSES];
    let mut n_done = 0usize;
    loop {
        let mut targets: [*mut EpollFile; MAX_TARGETS] = [core::ptr::null_mut(); MAX_TARGETS];
        let mut n_targets = 0usize;
        {
            let mut registry = epoll_wake_shard(file_id).lock_irqsave();
            // Intrusive-list walk (115th): read-only pass, no allocation,
            // no unlink — the scan cannot stall behind the allocator.
            // SAFETY: every node reachable from the sentinel is linked
            // under this shard lock, and nodes are only unlinked under it.
            let head = registry.init_once();
            let mut cur = unsafe { (*head).next };
            while cur != head {
                // SAFETY: cur is a linked node's `list` field (first
                // field of EpollWakeNode — the cast IS the container_of).
                let node = unsafe { &*(cur as *mut EpollWakeNode) };
                cur = unsafe { (*cur).next };
                if node.file_id != file_id {
                    continue;
                }
                let ep = node.ep;
                if done[..n_done].contains(&ep) {
                    continue; // already woken in an earlier pass
                }
                if n_targets < MAX_TARGETS {
                    // SAFETY: the binding exists under this shard lock, so
                    // epoll_file_close has not freed the instance yet (its
                    // free runs after unregister_all took this lock and
                    // removed this binding). Touching the pin field is
                    // therefore in-bounds live memory.
                    unsafe {
                        (*ep).notify_pins.fetch_add(1, core::sync::atomic::Ordering::Acquire);
                    }
                    targets[n_targets] = ep;
                    n_targets += 1;
                }
            }
        }
        if n_targets == 0 {
            break; // every watcher of this file_id has been processed
        }
        // SAFETY: each target is pinned (see the pin block above); the box
        // cannot be freed until the matching fetch_sub drops the last pin.
        for &ep in targets[..n_targets].iter() {
            let epoll = unsafe { &*ep };
            {
                let mut entries = epoll.entries.lock();
                // Intrusive-list walk (117th): read-modify pass, no
                // allocation — the re-arm cannot stall behind the
                // allocator while every other thread on this epoll spins.
                let head = entries.init_once();
                // SAFETY: every node reachable from the sentinel is linked
                // under this entries lock, and nodes are only unlinked
                // under it.
                let mut cur = unsafe { (*head).next };
                while cur != head {
                    // SAFETY: cur is a linked EpollEntry's `list` field
                    // (first field — the cast IS the container_of).
                    let entry = unsafe { &mut *(cur as *mut EpollEntry) };
                    cur = unsafe { (*cur).next };
                    if entry.file_id == file_id && entry.events & epoll_events::EPOLLET != 0 {
                        // New arrival = a fresh readiness edge, even if the
                        // previous one was reported and never observed idle.
                        entry.last_reported = 0;
                    }
                }
            }
            epoll.wait_queue.wake_up_all();
            // Release the pin AFTER the last touch of the instance.
            epoll.notify_pins.fetch_sub(1, core::sync::atomic::Ordering::Release);
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
fn epoll_wake_register(file_id: u64, ep: *mut EpollFile) {
    let shard = epoll_shard_index(file_id);
    // SAFETY: node is uniquely owned here; linked only under the shard
    // lock below (or freed on the idempotent path, still uniquely owned).
    let node = alloc::boxed::Box::into_raw(alloc::boxed::Box::new(EpollWakeNode {
        list: crate::list::ListHead::new(),
        file_id,
        ep,
    }));
    let mut linked = false;
    {
        let mut registry = EPOLL_WAKE_SHARDED[shard].lock_irqsave();
        let head = registry.init_once();
        let mut exists = false;
        // SAFETY: read-only walk of linked nodes under the shard lock.
        let mut cur = unsafe { (*head).next };
        while cur != head {
            // SAFETY: cur is a linked EpollWakeNode's `list` (first field).
            let n = unsafe { &*(cur as *mut EpollWakeNode) };
            if n.file_id == file_id && n.ep == ep {
                exists = true;
                break;
            }
            cur = unsafe { (*cur).next };
        }
        if !exists {
            // SAFETY: node's list is initialized (fresh) and unlinked;
            // head is this shard's sentinel, held under the lock.
            unsafe { (*node).list.add_tail(head); }
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
        // SAFETY: never linked; still uniquely owned.
        unsafe { drop(alloc::boxed::Box::from_raw(node)); }
    }
}

/// Drop every binding for `ep` (epoll instance teardown). An instance may
/// watch many file_ids across shards, so sweep them all (cold path: runs
/// once per epoll close).
///
/// Unlinked nodes are freed OUTSIDE the shard sections (batched): the free
/// can hit the allocator's slow path, which must never run under one of
/// these irqsave locks (see epoll_wake_register).
fn epoll_wake_unregister_all(ep: *mut EpollFile) {
    const BATCH: usize = 32;
    let mut batch: [*mut EpollWakeNode; BATCH] = [core::ptr::null_mut(); BATCH];
    for (shard, registry) in EPOLL_WAKE_SHARDED.iter().enumerate() {
        loop {
            let mut n_batch = 0usize;
            let mut hit_end;
            {
                let mut registry = registry.lock_irqsave();
                let head = registry.init_once();
                // SAFETY: walk under the shard lock; unlinking captures
                // `next` before `del`, and unlinked nodes are unreachable
                // to every other CPU once `del` lands under this lock.
                let mut cur = unsafe { (*head).next };
                hit_end = true;
                while cur != head {
                    let next = unsafe { (*cur).next };
                    // SAFETY: cur is a linked EpollWakeNode's `list`.
                    let n = unsafe { &mut *(cur as *mut EpollWakeNode) };
                    if n.ep == ep && n_batch < BATCH {
                        // SAFETY: n.list is linked in this shard's list.
                        unsafe { n.list.del(); }
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
            // Free the unlinked nodes with NO shard lock held.
            // SAFETY: each was unlinked under the shard lock above; nodes
            // are reachable only through the shard lists.
            for i in 0..n_batch {
                unsafe { drop(alloc::boxed::Box::from_raw(batch[i])); }
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
fn epoll_wake_unregister(file_id: u64, ep: *mut EpollFile) {
    let shard = epoll_shard_index(file_id);
    let mut dead: core::option::Option<*mut EpollWakeNode> = None;
    {
        let mut registry = EPOLL_WAKE_SHARDED[shard].lock_irqsave();
        let head = registry.init_once();
        // SAFETY: walk under the shard lock; the matching node is unlinked
        // and freed after the lock is released.
        let mut cur = unsafe { (*head).next };
        while cur != head {
            let next = unsafe { (*cur).next };
            // SAFETY: cur is a linked EpollWakeNode's `list`.
            let n = unsafe { &mut *(cur as *mut EpollWakeNode) };
            if n.file_id == file_id && n.ep == ep {
                // SAFETY: n.list is linked in this shard's list.
                unsafe { n.list.del(); }
                dead = Some(cur as *mut EpollWakeNode);
                EPOLL_WAKE_TOTAL.fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
                EPOLL_SHARD_N[shard].fetch_sub(1, core::sync::atomic::Ordering::Release);
                break;
            }
            cur = next;
        }
    }
    if let Some(n) = dead {
        // SAFETY: unlinked above; reachable only through the shard list.
        unsafe { drop(alloc::boxed::Box::from_raw(n)); }
    }
}

/// dfx=socksnap (b20 epoll wedge hunt): dump the epoll instances watching
/// `file_id` — one line per instance with owner, completed-wait count, and
/// the entry row (fd, events, fid, last_reported, reports). Called from the
/// Spawn-family connect trace (net::unix) because the khungtaskd cadence
/// that used to print epoll_interest_snapshot wedges after its FIRST pass
/// in every OH boot — this puts the counters on the connect timeline
/// instead. waits stuck at its pre-connect value across many connects is
/// the parked-forever signature; rep stuck at 0 while waits grows is the
/// never-reports signature. Lock order registry → entries (same as
/// epoll_notify_file); collect under lock, print after release.
pub fn epoll_dump_for_file(file_id: u64) {
    if !crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::SockSnap) {
        return;
    }
    let mut out: alloc::vec::Vec<(usize, u32, u64, alloc::vec::Vec<(i32, u32, u64, u32, u64)>)> =
        alloc::vec::Vec::new();
    {
        let mut registry = epoll_wake_shard(file_id).lock_irqsave();
        // SAFETY: read-only walk of linked nodes under the shard lock.
        let head = registry.init_once();
        let mut cur = unsafe { (*head).next };
        while cur != head {
            // SAFETY: cur is a linked EpollWakeNode's `list` (first field).
            let node = unsafe { &*(cur as *mut EpollWakeNode) };
            cur = unsafe { (*cur).next };
            if node.file_id != file_id {
                continue;
            }
            let ep = node.ep;
            let ep_usize = ep as usize;
            if out.iter().any(|(p, _, _, _)| *p == ep_usize) {
                continue; // one line per instance, not per binding
            }
            // SAFETY: `ep` is alive while we hold the registry lock (see
            // epoll_interest_snapshot).
            let epoll = unsafe { &*ep };
            let owner = epoll.owner_pid;
            let waits = epoll.wait_count;
            let mut row: alloc::vec::Vec<(i32, u32, u64, u32, u64)> =
                alloc::vec::Vec::with_capacity(24);
            {
                let mut entries = epoll.entries.lock();
                // Intrusive-list walk (117th): read-only pass, no
                // allocation under the entries lock (row is pre-sized).
                let head = entries.init_once();
                // SAFETY: read-only walk of nodes linked under this lock.
                let mut cur = unsafe { (*head).next };
                while cur != head {
                    // SAFETY: cur is a linked EpollEntry's `list` (first
                    // field).
                    let entry = unsafe { &*(cur as *mut EpollEntry) };
                    cur = unsafe { (*cur).next };
                    if row.len() < 24 {
                        row.push((
                            entry.fd,
                            entry.events,
                            entry.file_id,
                            entry.last_reported,
                            entry.reports,
                        ));
                    }
                }
            }
            out.push((ep_usize, owner, waits, row));
        }
    }
    for (ep, owner, waits, row) in out.iter() {
        let mut line: alloc::string::String = alloc::format!(
            "socksnap: EPWATCH fid={:#x} ep={:#x} owner={} waits={}",
            file_id,
            ep,
            owner,
            waits
        );
        for (fd, ev, fid, lr, rep) in row.iter() {
            line.push_str(&alloc::format!(
                " [fd={} ev={:#x} fid={:#x} lr={:#x} rep={}]",
                fd, ev, fid, lr, rep
            ));
        }
        crate::pr_warn!("{}", line);
    }
}

/// dfx=socksnap (appspawn wedge hunt, b15): dump the interest set of every
/// epoll instance reachable through the wake registry — one line per
/// instance listing up to 24 registered fds (fd number, events mask, file
/// id, ET snapshot). Cross-reference against the AF_UNIX listener dump
/// (net::unix::unix_listener_snapshot): a wedged server whose listener
/// file id appears in NO interest set had its registration removed (or
/// never made); one whose listener IS registered but never reports has a
/// readiness bug. Lock order registry → entries matches epoll_notify_file.
/// Formatting happens AFTER both locks drop (collect under lock, print
/// outside). Purely diagnostic; self-gated by dfx=socksnap.
pub fn epoll_interest_snapshot() {
    if !crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::SockSnap) {
        return;
    }
    // (epoll ptr, owner pid, wait count, [(fd, events, file_id, last_reported, reports); ..])
    // — collected under the registry + entries locks, printed after release.
    let mut out: alloc::vec::Vec<(
        usize,
        u32,
        u64,
        alloc::vec::Vec<(i32, u32, u64, u32, u64)>,
    )> = alloc::vec::Vec::new();
    {
        // Sharded since leg-31: sweep the shards one lock at a time
        // (never nested — same release-before-next discipline as the
        // original single-lock pass). Diagnostic-only path (socksnap).
        for shard in EPOLL_WAKE_SHARDED.iter() {
            let mut registry = shard.lock_irqsave();
            // SAFETY: read-only walk of linked nodes under the shard lock.
            let head = registry.init_once();
            let mut cur = unsafe { (*head).next };
            while cur != head {
                // SAFETY: cur is a linked EpollWakeNode's `list`.
                let node = unsafe { &*(cur as *mut EpollWakeNode) };
                cur = unsafe { (*cur).next };
                let ep = node.ep;
                let ep_usize = ep as usize;
                if out.iter().any(|(p, _, _, _)| *p == ep_usize) {
                    continue; // one line per instance, not per binding
                }
                // SAFETY: `ep` came from Box::into_raw in sys_epoll_create and
                // its close op unregisters the binding before freeing, so it is
                // alive while we hold the registry lock.
                let epoll = unsafe { &*ep };
                let owner = epoll.owner_pid;
                let waits = epoll.wait_count;
                let mut row: alloc::vec::Vec<(i32, u32, u64, u32, u64)> =
                    alloc::vec::Vec::with_capacity(24);
                let mut total: usize = 0;
                {
                    let mut entries = epoll.entries.lock();
                    // Intrusive-list walk (117th): read-only pass, no
                    // allocation under the entries lock (row is pre-sized;
                    // `total` counts every entry, past the 24-row cap).
                    let head = entries.init_once();
                    // SAFETY: read-only walk of nodes linked under this lock.
                    let mut cur = unsafe { (*head).next };
                    while cur != head {
                        // SAFETY: cur is a linked EpollEntry's `list`
                        // (first field).
                        let entry = unsafe { &*(cur as *mut EpollEntry) };
                        cur = unsafe { (*cur).next };
                        total += 1;
                        if row.len() < 24 {
                            row.push((
                                entry.fd,
                                entry.events,
                                entry.file_id,
                                entry.last_reported,
                                entry.reports,
                            ));
                        }
                    }
                }
                out.push((ep_usize, owner, waits, row));
                if total > 24 {
                    crate::pr_warn!("socksnap: ep={:#x} has {} entries (truncated)", ep_usize, total);
                }
            }
        }
    }
    for (ep, owner, waits, row) in out.iter() {
        let mut line: alloc::string::String = alloc::format!(
            "socksnap: ep={:#x} owner={} waits={} fds={}",
            ep,
            owner,
            waits,
            row.len()
        );
        for (fd, ev, fid, lr, rep) in row.iter() {
            line.push_str(&alloc::format!(
                " [fd={} ev={:#x} fid={:#x} lr={:#x} rep={}]",
                fd, ev, fid, lr, rep
            ));
        }
        crate::pr_info!("{}", line);
    }
}

/// Epoll file close callback
fn epoll_file_close(file: &crate::fs::File) -> i32 {
    // (review-s2b perf) this instance is gone — release the purge gate's
    // reference (see EPOLL_INSTANCES).
    EPOLL_INSTANCES.fetch_sub(1, core::sync::atomic::Ordering::Release);
    // R20-5: free the boxed EpollFile. The close op only runs on the LAST
    // Arc reference (close_fd decides under the entry lock), so no
    // epoll_ctl/epoll_wait can be dereferencing it — every path that
    // touches private_data holds an fd-table Arc clone. Previously the
    // Box (and its entries Vec) leaked on every epoll create+close.
    // SAFETY: private_data is an UnsafeCell; we hold &File for the last
    // reference, so no concurrent mutable access.
    if let Some(ptr) = unsafe { *file.private_data.get() } {
        // P1: purge this instance's wake-registry bindings BEFORE freeing
        // so epoll_notify_file can never touch a dangling queue.
        // SAFETY: ptr came from Box::into_raw in sys_epoll_create and is
        // uniquely owned by this File.
        unsafe {
            let epoll = &mut *(ptr as *mut EpollFile);
            epoll_wake_unregister_all(ptr as *mut EpollFile);
            // UAF drain (114th audit): a concurrent epoll_notify_file may
            // have collected this pointer under a shard lock just before
            // the unregister sweep removed the binding, and may still be
            // in its entries/wake section holding no shard lock. The pin
            // count it took under the shard lock is the only thing keeping
            // the box alive — wait for it to drain before freeing. No
            // locks are held here (unregister_all released every shard),
            // so this wait cannot deadlock; its duration is bounded by one
            // entries pass + one wake_up_all per pinned notify section.
            while epoll.notify_pins.load(core::sync::atomic::Ordering::Acquire) != 0 {
                core::hint::spin_loop();
            }
            // 117th: the interest nodes are separately-boxed `EpollEntry`s
            // now — unlink and free every one before the instance box
            // itself. Uncontended by construction (last Arc reference +
            // pin drain above), but the FREES still run OUTSIDE the
            // entries lock in bounded batches: a free can hit the
            // allocator's slow path, and a preempt-off section of
            // hundreds of frees is exactly the monopolist window shape
            // that stalls every concurrent RCU grace round (the storm
            // family this port eliminates elsewhere).
            {
                const CLOSE_BATCH: usize = 32;
                let mut batch: [*mut EpollEntry; CLOSE_BATCH] =
                    [core::ptr::null_mut(); CLOSE_BATCH];
                loop {
                    let mut n_batch = 0usize;
                    let mut hit_end;
                    {
                        let mut entries = epoll.entries.lock();
                        let head = entries.init_once();
                        // SAFETY: walk under the entries lock; `next` is
                        // captured before the unlink, and an unlinked node
                        // is unreachable to every other CPU once `del`
                        // lands under this lock.
                        let mut cur = unsafe { (*head).next };
                        hit_end = true;
                        while cur != head {
                            let next = unsafe { (*cur).next };
                            // SAFETY: cur is a linked EpollEntry's `list`
                            // (first field); the node was Box::into_raw'd
                            // in sys_epoll_ctl and is reachable only
                            // through this list.
                            let node = unsafe { &mut *(cur as *mut EpollEntry) };
                            if n_batch < CLOSE_BATCH {
                                unsafe { node.list.del(); }
                                batch[n_batch] = node as *mut EpollEntry;
                                n_batch += 1;
                            } else {
                                hit_end = false;
                                break;
                            }
                            cur = next;
                        }
                    }
                    // Free the batch with NO entries lock held.
                    // SAFETY: each node was unlinked under the lock above;
                    // interest nodes are never shared.
                    for i in 0..n_batch {
                        unsafe { drop(alloc::boxed::Box::from_raw(batch[i])); }
                        batch[i] = core::ptr::null_mut();
                    }
                    if hit_end {
                        break;
                    }
                }
            }
            let _ = alloc::boxed::Box::from_raw(ptr as *mut EpollFile);
        }
        unsafe { *file.private_data.get() = None; }
    }
    0
}

/// Epoll file operations (D-6 builder form: epoll carries ONLY close —
/// no read/write/seek/poll capability; read() on an epoll fd takes the
/// central EINVAL path, the §1 incident's fd family).
static EPOLL_OPS: crate::fs::FileOps = crate::fs::FileOps::builder()
    .close(epoll_file_close)
    .build();

/// D-6 shape-test accessors: expose the (otherwise private) converted
/// ops tables to the unit-test group `fileops-caps`
/// (kernel/src/tests/fileops_caps.rs) without widening the runtime API.
#[cfg(any(test, feature = "unit-test"))]
pub mod caps_shape {
    pub fn epoll_ops() -> &'static crate::fs::FileOps {
        &super::EPOLL_OPS
    }
    pub fn eventfd_ops() -> &'static crate::fs::FileOps {
        &super::EVENTFD_OPS
    }
    pub fn timerfd_ops() -> &'static crate::fs::FileOps {
        &super::TIMERFD_OPS
    }
    pub fn signalfd_ops() -> &'static crate::fs::FileOps {
        &super::SIGNALFD_OPS
    }
    pub fn pipe_ops() -> &'static crate::fs::FileOps {
        &crate::fs::pipe::PIPE_OPS
    }
    pub fn inotify_ops() -> &'static crate::fs::FileOps {
        &crate::fs::inotify::INOTIFY_OPS
    }
}

/// oh67 (67th session): diagnostic readiness decode for census poll
/// rows — the KERNEL-SIDE view of the fd an infinite-timeout poll is
/// parked on. eventfd: the live counter (counter>0 while a POLLIN poll
/// sleeps = a lost-wake kernel bug; counter==0 = the producer never
/// wrote — upstream starvation). timerfd: unread expiration count +
/// interval. pipe: readable bytes + closed flags. Diagnostic only;
/// called from the sleepsnap/TXN-HANG census contexts (bounded reads).
pub fn oh67_fd_ready(file: &crate::fs::File) -> Option<alloc::string::String> {
    use core::sync::atomic::Ordering::Relaxed;
    let ops = unsafe { *file.ops.get() }?;
    if core::ptr::eq(ops as *const _, &EVENTFD_OPS as *const _) {
        let ptr = unsafe { *file.private_data.get() }?;
        // SAFETY: dispatcher identity-checked against EVENTFD_OPS;
        // private_data came from Box::into_raw in sys_eventfd2.
        let efd = unsafe { &*(ptr as *const EventFd) };
        return Some(alloc::format!(
            "eventfd cnt={}",
            efd.counter.load(Relaxed)
        ));
    }
    if core::ptr::eq(ops as *const _, &TIMERFD_OPS as *const _) {
        let ptr = unsafe { *file.private_data.get() }?;
        // SAFETY: dispatcher identity-checked against TIMERFD_OPS;
        // private_data came from Box::into_raw in sys_timerfd_create.
        let tfd = unsafe { &*(ptr as *const TimerFd) };
        return Some(alloc::format!(
            "timerfd exp={} interval_j={} tmr={}",
            tfd.expiration_count.load(Relaxed),
            tfd.interval_jiffies,
            tfd.kernel_timer_id
        ));
    }
    if crate::fs::pipe::is_pipe_file(file) {
        if let Some((readable, wclosed)) = crate::fs::pipe::oh67_pipe_ready(file) {
            return Some(alloc::format!("pipe rd={} wclosed={}", readable, wclosed));
        }
    }
    if let Some((name, segs, charge, peer, accept_n)) = crate::net::unix::oh67_unix_ready(file)
    {
        return Some(alloc::format!(
            "unix[{}] segs={} charge={} peer={} acc={}",
            name, segs, charge, peer, accept_n
        ));
    }
    None
}

/// sys_poll - I/O multiplexing (poll style)
///
/// # Arguments
/// - args[0]: fds - pointer to pollfd array
/// - args[1]: nfds - length of pollfd array
/// - args[2]: timeout - timeout in milliseconds
///
/// # Returns
/// Returns number of ready file descriptors on success, 0 on timeout, negative error code on failure
/// High-resolution ns deadline for a relative poll/select/epoll timeout.
/// LTP timer-accuracy tests (nanosleep01/select02/poll02) measure the
/// elapsed wall time and fail ANY early return: the old jiffy-floor
/// arithmetic (ns -> ms floor -> jiffies floor) turned a 1 ms timeout
/// into an instant return. The deadline gates the loop EXIT; the jiffy
/// sleeps inside the loop only decide WHEN to re-check, so a tick that
/// lands early just loops again.
fn poll_deadline_ns(timeout_ns: i64) -> Option<u64> {
    if timeout_ns <= 0 {
        return None;
    }
    Some(
        crate::drivers::timer::monotonic_ns_since_boot().saturating_add(timeout_ns as u64),
    )
}

/// Jiffy-grid deadline that never falls before the ns deadline (ceil of
/// the remaining ns, minimum 1 jiffy so a sub-tick wait still re-checks
/// at the next tick instead of spinning).
/// Convert an absolute monotonic-ns deadline into an ABSOLUTE time-CSR
/// tick deadline for hres wake timers (+1 guard tick so the wake is never
/// early; never armed in the past). Same timebase as
/// monotonic_ns_since_boot() (read_time-derived), so the conversion is
/// exact — this is what lets a 1 ms poll timeout actually wake at ~1 ms
/// instead of the next 10 ms grid tick (LTP epoll_wait02/pselect01
/// "slept for too long", median 9998us for a 1000us request).
pub fn deadline_ns_to_hres_ticks(deadline_ns: u64) -> u64 {
    let freq_hz = crate::config::TIMER_CLOCK_FREQ_HZ as u128;
    let ticks = (deadline_ns as u128 * freq_hz / 1_000_000_000u128) as u64;
    let now = crate::drivers::timer::read_time();
    ticks.saturating_add(1).max(now + 1)
}

pub fn jiffies_covering_deadline(deadline_ns: u64) -> u64 {
    let now_ns = crate::drivers::timer::monotonic_ns_since_boot();
    let rem_ns = deadline_ns.saturating_sub(now_ns);
    let j = ((rem_ns as u128 * crate::drivers::timer::HZ as u128)
        .div_ceil(1_000_000_000u128)) as u64;
    crate::drivers::timer::get_jiffies().saturating_add(j.max(1))
}

pub fn sys_poll(args: SyscallArgs) -> i64 {
    use poll_events::*;

    let fds_ptr = args[0] as *mut PollFd;
    let nfds = args[1] as usize;
    let timeout_ms = args[2] as i32;

    // Reference do_sys_poll bounds nfds by the caller's RLIMIT_NOFILE
    // soft limit BEFORE touching anything — poll(nfds=-1) is EINVAL, not
    // a 32 GB kernel allocation (LTP ppoll01 INVALID_NFDS used to hit
    // bigalloc REJECTED -> task-context-ENOMEM SIGKILL here). ppoll and
    // ppoll_time64 both delegate to this core, so the bound covers all
    // three entry points. RLIM_INFINITY (u64::MAX) means uncapped.
    {
        let nofile = crate::sched::current()
            // irqsave: current's rlimits lock is shared with IRQ context
            // (scheduler_tick -> Task::rlimit -> lock_irqsave); a plain
            // holder interrupted by the tick self-deadlocks the CPU.
            .map(|t| unsafe { (*t).rlimits.lock_irqsave()[crate::process::task::rlimit_res::NOFILE].0 })
            .unwrap_or(1024);
        if nfds as u64 > nofile {
            return -(errno::EINVAL as i64);
        }
    }

    // nfds == 0 with a NULL array is the classic poll(NULL, 0, ms) sleep
    // idiom — legal on Linux. Only a NULL array with nfds != 0 is EFAULT.
    let fds_size = core::mem::size_of::<PollFd>().saturating_mul(nfds);
    if fds_ptr.is_null() {
        if nfds != 0 {
            return -(errno::EFAULT as i64);
        }
    } else if fds_size == 0 {
        // Overflowed (Linux does not cap nfds): reject rather than wrap.
        if nfds != 0 {
            return -(errno::EINVAL as i64);
        }
    } else {
        if !crate::arch::uaccess::access_ok(fds_ptr as usize, fds_size) {
            return -(errno::EFAULT as i64);
        }
    }

    // Get current process fdtable
    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(errno::EBADF as i64),
    };

    // Allocation discipline (s2-a review finding P1): the scan loop
    // below can run many rounds (every spurious wake re-enters it);
    // the pollfd mirror and the resolved-file array are allocated ONCE
    // here and reused per round. nfds is already bounded (<= 1024), so
    // these are bounded buffers.
    let mut pollfds = alloc::vec![PollFd { fd: -1, events: 0, revents: 0 }; nfds];
    let mut files: alloc::vec::Vec<alloc::sync::Arc<crate::fs::File>> =
        alloc::vec::Vec::with_capacity(nfds);

    // Poll loop with timeout support. The expiry decision runs on the
    // high-res ns clock (see poll_deadline_ns); jiffies only schedule
    // the intermediate re-check sleeps.
    let deadline_ns = poll_deadline_ns(timeout_ms as i64 * 1_000_000);

    loop {
        let mut ready_count = 0usize;

        // Check all file descriptors. The pollfd array is USER memory:
        // copy it in, compute revents, copy it back — all through the
        // exception-table paths (SUM=0 safe; raw derefs fault the kernel).
        // SAFETY: fds_ptr validated with access_ok; nfds bounded to 1024.
        // (buffer hoisted out of the loop — reused per round)
        unsafe {
            crate::arch::uaccess::copy_from_user(
                pollfds.as_mut_ptr() as *mut u8,
                fds_ptr as *const u8,
                fds_size,
            );

            for i in 0..nfds {
                let pollfd = &mut pollfds[i];
                pollfd.revents = 0;

                // POSIX poll(2): "If the value of fd is negative, the
                // events field is ignored and the revents field is set
                // to 0." GLib pads its poll arrays with fd=-1 slots for
                // sources removed between iterations and RELIES on this:
                // reporting POLLNVAL for them made every glib main-loop
                // iteration return "1 fd ready" and spin forever at 100%
                // CPU (gnome-session-binary wedged in ppoll — the gsd
                // component discovery regression).
                if pollfd.fd < 0 {
                    continue;
                }

                let file = match fdtable.get_file(pollfd.fd as usize) {
                    Some(f) => f,
                    None => {
                        pollfd.revents |= POLLNVAL;
                        ready_count += 1;
                        continue;
                    }
                };

                // Central dispatch (D-6): per-file-type poll callback
                // when the type declared the capability, else the
                // always-ready fallback — one place, not per-callsite.
                let revents = file.poll_dispatch(pollfd.events);

                // Linux do_pollfd(): revents are MASKED to the requested
                // events plus the always-reportable ERR/HUP/NVAL. Drivers
                // return readiness in the *NORM companions (e.g. a unix
                // socket reports POLLIN|POLLRDNORM) and the mask strips
                // the bits the caller never asked for. libxcb treats any
                // revents bit outside its requested set as a fatal poll
                // error, so leaking POLLRDNORM here killed every X client
                // handshake ("Can't open display").
                let revents =
                    revents & (pollfd.events | POLLERR | POLLHUP | POLLNVAL);

                if revents != 0 {
                    pollfd.revents = revents;
                    ready_count += 1;
                }
            }

            // SAFETY: fds_ptr validated with access_ok(fds_size) above.
            crate::arch::uaccess::copy_to_user(
                fds_ptr as *mut u8,
                pollfds.as_ptr() as *const u8,
                fds_size,
            );
        }

        if ready_count > 0 {
            return ready_count as i64;
        }

        // No fd ready - check timeout
        if timeout_ms == 0 {
            return 0;  // Return immediately
        }

        // timeout_ms < 0 means wait forever (only break on data or signal)
        if timeout_ms > 0 {
            // Check if timeout expired (high-resolution; never early)
            if let Some(dl) = deadline_ns {
                if crate::drivers::timer::monotonic_ns_since_boot() >= dl {
                    return 0;
                }
            }
        }

        // Check for pending signals
        if crate::signal::signal_pending() {
            return -(errno::EINTR as i64);
        }

        // P3 (event-driven wait): register on every polled file's readiness
        // queue and sleep until a producer event (or the timeout timer, or a
        // signal) — instead of waking at tick rate to re-poll. This is what
        // keeps idle GLib/Xorg/dbus main loops at 0% CPU. Falls back to the
        // old timed re-check slice (P1) whenever any polled fd has no wake
        // path registered (poll_wait_impls None), preserving behavior for
        // exotic fd types.
        {
            files.clear();
            let mut wired = true;
            for i in 0..nfds {
                let pollfd = &pollfds[i];
                if pollfd.fd < 0 {
                    // Ignored slot (glib pads with fd=-1): no wait target.
                    continue;
                }
                match fdtable.get_file(pollfd.fd as usize) {
                    Some(f) => files.push(f),
                    None => {
                        // fd closed under us: the next scan reports
                        // POLLNVAL — fall back to one slice round.
                        wired = false;
                        break;
                    }
                }
            }
            if wired && !files.is_empty() {
                let task = match crate::sched::current() {
                    Some(t) => t,
                    None => return -(errno::EINTR as i64),
                };
                // Quick readiness re-check used inside the wait round: any
                // polled fd reporting nonzero revents ends the sleep.
                let pollfds_ref = &pollfds;
                let fdtable_ref = &fdtable;
                let quick_ready = || -> bool {
                    for pfd in pollfds_ref.iter() {
                        if pfd.fd < 0 {
                            continue;
                        }
                        let f = match fdtable_ref.get_file(pfd.fd as usize) {
                            Some(f) => f,
                            None => return true, // POLLNVAL
                        };
                        // Central dispatch (D-6): capability fn or the
                        // always-ready fallback; mask + decide here.
                        let revents = f.poll_dispatch(pfd.events);
                        if revents & (pfd.events | POLLERR | POLLHUP | POLLNVAL) != 0 {
                            return true;
                        }
                    }
                    false
                };
                match poll_event_wait_round(
                    &files,
                    task,
                    deadline_ns,
                    &quick_ready,
                ) {
                    PollEventWait::Interrupted => return -(errno::EINTR as i64),
                    PollEventWait::Woken => continue,
                    PollEventWait::NotWired => {} // fall through to the slice
                }
            }
        }

        // P1 (busy-yield elimination, same discipline as sys_epoll_wait):
        // block for one 10 ms re-check slice instead of yield_cpu()-ing.
        // The tight yield loop livelocked the machine under SMP — every
        // iteration took the global runqueue lock against every CPU's
        // scheduler_tick (xkbcomp's ppoll ground three harts into a GRQ
        // spinlock storm and Xorg never progressed past XKB init).
        poll_sleep_slice(deadline_ns);
    }
}

/// Block the calling task for one poll/select re-check slice (min of
/// remaining timeout and EPOLL_RECHECK_JIFFIES), timer-woken. Degrades to
/// a single schedule() round when the timer pool is exhausted.
fn poll_sleep_slice(deadline_ns: Option<u64>) {
    use crate::process::task::TaskState;

    let current = match crate::sched::current() {
        Some(t) => t,
        None => {
            crate::sched::yield_cpu();
            return;
        }
    };
    // SAFETY: current is the running task's pointer.
    let my_pid = unsafe { (*current).pid() };
    let slice_ns: u64 = EPOLL_RECHECK_JIFFIES
        .saturating_mul(1_000_000_000 / crate::drivers::timer::HZ as u64);
    // Deadline precision (nanosleep discipline): when the remaining wait
    // fits inside one re-check slice, arm a HIGH-RESOLUTION wake at the
    // precise ns deadline (between grid ticks); earlier slices keep the
    // jiffy cadence. Without this the final slice rounded a <=10 ms
    // timeout up to the next 10 ms grid tick — a 1 ms poll timeout slept
    // ~10 ms (LTP epoll_wait02/pselect01 precision budgets).
    let remaining_ns = deadline_ns.map(|dl| {
        dl.saturating_sub(crate::drivers::timer::monotonic_ns_since_boot())
    });
    let timer_id = match deadline_ns {
        // Remaining wait fits inside one slice (or already due): precise
        // hres wake at the ns deadline (deadline_ns_to_hres_ticks clamps
        // an already-due deadline to now+1, firing at once).
        Some(dl) if remaining_ns.unwrap_or(u64::MAX) <= slice_ns => {
            let id = crate::timer::add_timer_wakeup_hres(
                jiffies_covering_deadline(dl),
                deadline_ns_to_hres_ticks(dl),
                my_pid,
            );
            crate::drivers::timer::rearm_for_hres();
            id
        }
        // Longer than a slice (or no deadline): plain jiffy-cadence slice.
        _ => crate::timer::add_timer_wakeup(
            crate::drivers::timer::get_jiffies() + EPOLL_RECHECK_JIFFIES,
            my_pid,
        ),
    };
    if timer_id == 0 {
        // Timer pool exhausted: degrade to one scheduling round rather
        // than sleeping forever (nothing else is guaranteed to wake us).
        crate::sched::yield_cpu();
        return;
    }
    // SAFETY: current is the running task's pointer.
    unsafe {
        (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
    }
    // Lost-wake discipline (khungtaskd pattern): the timer may have fired
    // between arming and the state transition — either its wake captured
    // the INTERRUPTIBLE state, or the deadline is already visible. With a
    // deadline the check is the precise ns clock (the hres wake may be
    // mid-tick); without one the slice never "expires" early.
    let already_due = match deadline_ns {
        Some(dl) => crate::drivers::timer::monotonic_ns_since_boot() >= dl,
        None => false,
    };
    if already_due {
        // SAFETY: current is the running task's pointer; a racing wake may
        // have enqueued us while still executing — take ourselves back off.
        unsafe {
            (*current).set_state(TaskState::new(TaskState::RUNNING));
            crate::sched::dequeue_task(&*current);
        }
        crate::timer::del_timer(timer_id);
    } else {
        // Enable interrupts so the tick can reach us, then sleep. Syscall
        // context runs with SIE=0; __schedule must see SIE=1 (same
        // contract as wait_event! / sys_epoll_wait).
        crate::arch::cpu::restore_irq(true);
        crate::sched::schedule();
        // Spurious wake before expiry: drop the timer so it cannot enqueue
        // us again while running.
        if crate::timer::timer_pending(timer_id) {
            crate::timer::del_timer(timer_id);
        }
    }
}

// ============================================================================
// Event-driven poll waiting (P3: poll_wait queue registration)
// ============================================================================

/// Per-fd-type poll-wait registration pair, dispatched by FileOps identity.
type PollWaitRegFn = fn(&crate::fs::File, *mut crate::process::task::Task) -> bool;
type PollWaitUnregFn = fn(&crate::fs::File, *mut crate::process::task::Task);

/// Resolve the poll-wait register/unregister pair for `file`, or None when
/// the file type has no producer-woken readiness queue.
///
/// This is the poll_wait(2) analogue of Linux: instead of waking every
/// 10ms to re-poll every fd (the old P1 slice, which made each idle
/// GLib/Xorg/dbus main loop burn CPU at tick rate — and, with ~40 such
/// waiters in a GNOME session, oversubscribed every CPU), a poll/select/
/// epoll_wait blocker registers on the target's readiness queue and sleeps
/// until a real producer event (data arrival, space freed, EOF, connect
/// queued, inotify event, input event, timer expiry, signal arrival).
fn poll_wait_impls(file: &crate::fs::File) -> Option<(PollWaitRegFn, PollWaitUnregFn)> {
    let ops = file.get_ops()? as *const crate::fs::FileOps as *const ();
    let is = |p: &crate::fs::FileOps| ops == (p as *const _ as *const ());
    if is(&crate::net::unix::UNIX_SOCKET_OPS) {
        return Some((
            crate::net::unix::unix_poll_wait,
            crate::net::unix::unix_poll_unwait,
        ));
    }
    if is(&crate::fs::pipe::PIPE_OPS) || is(&crate::fs::fifo::FIFO_OPS) {
        return Some((
            crate::fs::pipe::pipe_poll_wait,
            crate::fs::pipe::pipe_poll_unwait,
        ));
    }
    if is(&crate::net::socket::SOCKET_OPS) {
        return Some((
            crate::net::socket::socket_poll_wait,
            crate::net::socket::socket_poll_unwait,
        ));
    }
    if is(&crate::fs::inotify::INOTIFY_OPS) {
        return Some((
            crate::fs::inotify::inotify_poll_wait,
            crate::fs::inotify::inotify_poll_unwait,
        ));
    }
    // /dev/binder: reference binder_poll registers the polling tid on its
    // per-thread binder wait queue (26th session reference-parity port);
    // without this arm any poll()/epoll_wait() over a binder fd fell back
    // to the 10 ms re-check slice — latency plus a full fd-set rescan per
    // slice per waiter.
    if is(&crate::ipc::binder::BINDER_OPS) {
        return Some((
            crate::ipc::binder::binder_poll_wait,
            crate::ipc::binder::binder_poll_unwait,
        ));
    }
    if is(&crate::drivers::input::evdev::EVDEV_OPS) {
        return Some((
            crate::drivers::input::evdev::evdev_poll_wait,
            crate::drivers::input::evdev::evdev_poll_unwait,
        ));
    }
    if is(&TIMERFD_OPS) {
        return Some((timerfd_poll_wait, timerfd_poll_unwait));
    }
    if is(&super::EVENTFD_OPS) {
        return Some((eventfd_poll_wait, eventfd_poll_unwait));
    }
    // pidfds (pidfd_open files and /proc/<pid> dir fds): exit readiness
    // wakes through the global pidfd queue (do_exit notifies).
    if is(&crate::fs::pidfd::PIDFD_OPS)
        || is(&crate::fs::procfs::PROC_PIDDIR_OPS)
    {
        return Some((
            crate::fs::pidfd::pidfd_poll_wait,
            crate::fs::pidfd::pidfd_poll_unwait,
        ));
    }
    if is(&SIGNALFD_OPS) {
        return Some((signalfd_poll_wait, signalfd_poll_unwait));
    }
    if is(&crate::perf::PERF_OPS) {
        return Some((crate::perf::perf_poll_wait, crate::perf::perf_poll_unwait));
    }
    None
}


// Raw-console writers for the dfx=epollscan trace (leg-9 epoll hunt):
// pr_info is dropped whole at session loglevel=3, so the scan trace uses
// the same putchar_no_lock path as sched/wakewatch's ww_msg and evdev's
// INPTRC/INPPL lines.
fn eps_msg(msg: &[u8]) {
    for &b in msg {
        crate::console::putchar_no_lock(b);
    }
}

fn eps_u64(mut v: u64) {
    let mut buf = [0u8; 20];
    let mut n = 0;
    if v == 0 {
        buf[0] = b'0';
        n = 1;
    } else {
        while v > 0 && n < buf.len() {
            buf[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
    }
    for i in (0..n).rev() {
        crate::console::putchar_no_lock(buf[i]);
    }
}

/// Result of one event-driven wait round.
enum PollEventWait {
    /// Slept and was woken (event, timeout, or spurious): re-check readiness.
    Woken,
    /// At least one polled file has no wake path (or the timer pool was
    /// exhausted): the caller must fall back to a timed re-check slice.
    NotWired,
    /// A signal is pending: return EINTR.
    Interrupted,
}

/// One event-driven sleep round across `files` (every fd the caller is
/// waiting on): register on each file's readiness queue, re-check
/// `ready()` under the registration (lost-wake discipline), arm a timeout
/// timer for the full remaining deadline, and sleep until woken.
///
/// SAFETY contract: `task` is the current task; the caller keeps every
/// Arc<File> in `files` alive for the duration (they are cloned from the
/// caller's fdtable), so the private_data objects behind them outlive the
/// registrations.
fn poll_event_wait_round(
    files: &[alloc::sync::Arc<crate::fs::File>],
    task: *mut crate::process::task::Task,
    // ABSOLUTE monotonic-ns deadline (None = wait forever). Armed as a
    // HIGH-RESOLUTION wake (same discipline as nanosleep) so short
    // timeouts fire between grid ticks, not at the next 10 ms jiffy.
    deadline: Option<u64>,
    ready: &dyn Fn() -> bool,
) -> PollEventWait {
    use crate::process::task::TaskState;
    use crate::process::wait::WaitQueueEntry;

    // 1. State transition BEFORE registering on the queues (the
    //    prepare_to_wait discipline: a producer wake must never find the
    //    task RUNNING with entries already on the queue — wake_up_process
    //    would take the R46 refuse path and merely mark the entry woken;
    //    with the state already INTERRUPTIBLE the wake enqueues us and the
    //    re-check below catches the readiness). A wake racing with the
    //    registration window enqueues us; step 3's ready() re-check then
    //    returns without sleeping (dequeue_task undoes the racing enqueue).
    // SAFETY: task is the current task.
    unsafe {
        (*task).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
    }

    // 2. Resolve + register on every file's queues. Any file without a
    //    wake path forces the caller back to the timed-slice fallback
    //    (its readiness must still be re-checked periodically).
    let mut regs: alloc::vec::Vec<PollWaitUnregFn> = alloc::vec::Vec::new();
    for f in files.iter() {
        match poll_wait_impls(f) {
            Some((reg, unreg)) => {
                if !reg(f, task) {
                    for (i, r) in regs.iter().enumerate() {
                        r(&files[i], task);
                    }
                    // SAFETY: restore RUNNING before returning (we never
                    // slept; a racing wake may have enqueued us — undo).
                    unsafe {
                        (*task).set_state(TaskState::new(TaskState::RUNNING));
                        crate::sched::dequeue_task(&*task);
                    }
                    return PollEventWait::NotWired;
                }
                regs.push(unreg);
            }
            None => {
                for (i, r) in regs.iter().enumerate() {
                    r(&files[i], task);
                }
                // SAFETY: see above.
                unsafe {
                    (*task).set_state(TaskState::new(TaskState::RUNNING));
                    crate::sched::dequeue_task(&*task);
                }
                return PollEventWait::NotWired;
            }
        }
    }

    let unwait_all = |task: *mut crate::process::task::Task| {
        for (i, r) in regs.iter().enumerate() {
            r(&files[i], task);
        }
    };

    // 3. Re-check readiness AFTER registering (lost-wake discipline): an
    //    event that landed between the caller's readiness scan and the
    //    queue registration is caught here.
    if ready() {
        unwait_all(task);
        // SAFETY: current is the running task's pointer; a racing wake may
        // have enqueued us while still executing — take ourselves back off
        // (NEW-C2 discipline).
        unsafe {
            (*task).set_state(TaskState::new(TaskState::RUNNING));
            crate::sched::dequeue_task(&*task);
        }
        return PollEventWait::Woken;
    }

    if crate::signal::signal_pending() {
        unwait_all(task);
        // SAFETY: see above.
        unsafe {
            (*task).set_state(TaskState::new(TaskState::RUNNING));
            crate::sched::dequeue_task(&*task);
        }
        return PollEventWait::Interrupted;
    }

    // 4. Arm the timeout timer for the FULL remaining wait (no re-check
    //    slice: fd-side events are the wake source now). HIGH-RESOLUTION:
    //    the precise ns deadline rides the time CSR (between grid ticks);
    //    the jiffy value is the wheel bookkeeping/fallback.
    let timer_id = deadline
        .map(|dl| {
            let id = crate::timer::add_timer_wakeup_hres(
                jiffies_covering_deadline(dl),
                deadline_ns_to_hres_ticks(dl),
                // SAFETY: task is the current task; pid() is a read.
                unsafe { (*task).pid() },
            );
            // We are on the CPU that is about to sleep: re-arm the local
            // timer so the precise deadline is not delayed to the next
            // grid tick (nanosleep discipline).
            crate::drivers::timer::rearm_for_hres();
            id
        })
        .unwrap_or(0);
    if deadline.is_some() && timer_id == 0 {
        // Timer pool exhausted: a timed wait would sleep forever — fall
        // back to the slice path for this round.
        unwait_all(task);
        // SAFETY: see above.
        unsafe {
            (*task).set_state(TaskState::new(TaskState::RUNNING));
        }
        return PollEventWait::NotWired;
    }

    // 5. Sleep. Syscall context runs with SIE=0; __schedule must see
    //    SIE=1 (same contract as wait_event! / sys_epoll_wait).
    crate::arch::cpu::restore_irq(true);
    crate::sched::schedule();

    // 6. Cleanup: drop the timer so it cannot re-enqueue us while running,
    //    remove every registration, restore RUNNING if we are still asleep
    //    (a timeout/spurious wake path may have left the state).
    if timer_id != 0 && crate::timer::timer_pending(timer_id) {
        crate::timer::del_timer(timer_id);
    }
    unwait_all(task);
    // SAFETY: task is the current task.
    unsafe {
        if (*task).state().is_sleeping() {
            (*task).set_state(TaskState::new(TaskState::RUNNING));
        }
    }
    if crate::signal::signal_pending() {
        return PollEventWait::Interrupted;
    }
    PollEventWait::Woken
}

/// RAII restore for a wait-scoped signal mask (ppoll/pselect6). The
/// caller's mask is restored on EVERY exit path — return value, timeout,
/// EINTR, error — matching the reference's set_restore_sigmask discipline
/// where the swap + wait + restore is one atomic operation from the
/// caller's viewpoint. On the EINTR exit the restore is DEFERRED instead
/// (defer_restore): the wait mask stays active so the interrupting signal
/// is deliverable at return-to-user, and the saved mask is reinstated by
/// the delivery path after the handler runs (reference -ERESTARTNOHAND +
/// restore_saved_sigmask — the same discipline rt_sigsuspend uses).
struct WaitScopedMask {
    saved: u64,
    live: bool,
}

impl WaitScopedMask {
    /// Install `new_mask` as the running task's blocked set. SIGKILL/
    /// SIGSTOP are cleared (unblockable — same rule as rt_sigprocmask
    /// and epoll_pwait's scoped mask).
    fn install(new_mask: u64) -> Option<Self> {
        let current = crate::sched::current()?;
        // SAFETY: current is the running task; sigmask is a plain u64
        // field written only by the task itself (syscall context).
        let saved = unsafe { (*current).sigmask };
        unsafe { (*current).sigmask = new_mask & !((1u64 << 8) | (1u64 << 18)); }
        Some(WaitScopedMask { saved, live: true })
    }

    /// Signal-interrupted exit: hand the restore to the signal-delivery
    /// path (sigmask_restore) instead of restoring here, so the pending
    /// interrupting signal — unblocked by the wait mask — is selected and
    /// delivered, and the handler runs with the reference's mask state.
    fn defer_restore(mut self) {
        if self.live {
            self.live = false;
            if let Some(current) = crate::sched::current() {
                // SAFETY: see install().
                unsafe {
                    (*current).sigmask_restore = self.saved;
                    (*current).sigmask_restore_valid = true;
                }
            }
        }
    }
}

impl Drop for WaitScopedMask {
    fn drop(&mut self) {
        if self.live {
            self.live = false;
            if let Some(current) = crate::sched::current() {
                // SAFETY: see install().
                unsafe { (*current).sigmask = self.saved; }
            }
        }
    }
}

/// sys_ppoll - I/O multiplexing (ppoll style, syscall nr=73)
///
/// # Arguments
/// - args[0]: fds - pointer to pollfd array
/// - args[1]: nfds - length of pollfd array
/// - args[2]: timeout - pointer to struct timespec (sec, nsec), or NULL for infinite
/// - args[3]: sigmask - pointer to signal mask replacing the blocked set
///   for the duration of the wait, or NULL to keep the current mask
/// - args[4]: sigsetsize - must be sizeof(sigset_t) (8)
///
/// # Returns
/// Returns number of ready file descriptors on success, 0 on timeout, negative error code on failure
pub fn sys_ppoll(args: SyscallArgs) -> i64 {
    // Reference SYSCALL_DEFINE5(ppoll) rejects a wrong sigsetsize before
    // anything else; glibc/musl always pass 8 on x86_64.
    if args[4] as usize != 8 {
        return -(errno::EINVAL as i64);
    }
    // ppoll has same pollfd checking logic as poll, but reads timeout from timespec
    let timeout_ptr = args[2] as *const u64;

    // A NULL timeout means "wait forever" (legal); a non-NULL but invalid
    // pointer must fail with EFAULT instead of silently waiting forever.
    if !timeout_ptr.is_null()
        && !crate::arch::uaccess::access_ok(timeout_ptr as usize, 16)
    {
        return -(errno::EFAULT as i64);
    }

    // Read timeout from struct timespec { tv_sec: u64, tv_nsec: u64 }
    let timeout_ms: i32 = if timeout_ptr.is_null() {
        -1  // NULL = infinite wait
    } else {
        // SAFETY: timeout_ptr validated with access_ok; get_user is the
        // exception-table copy path (SUM=0 safe). An unreadable page must
        // be EFAULT — the old unwrap_or(0) turned it into {0,0} and made
        // ppoll() return immediately instead of failing (reference
        // do_sys_poll: copy_from_user of the timespec fails with EFAULT).
        unsafe {
            let tv_sec = match crate::arch::uaccess::get_user(timeout_ptr) {
                Some(v) => v,
                None => return -(errno::EFAULT as i64),
            };
            let tv_nsec = match crate::arch::uaccess::get_user(timeout_ptr.add(1)) {
                Some(v) => v,
                None => return -(errno::EFAULT as i64),
            };
            if tv_nsec >= 1_000_000_000 {
                // Invalid timespec: match Linux poll_select_set_timeout().
                return -(errno::EINVAL as i64);
            }
            if tv_sec == 0 && tv_nsec == 0 {
                0  // Immediate return
            } else {
                // Convert to milliseconds, cap at i32 max
                let total_ms = tv_sec.saturating_mul(1000).saturating_add(tv_nsec / 1_000_000);
                if total_ms > i32::MAX as u64 {
                    -1  // Very long timeout = infinite for our purposes
                } else {
                    total_ms as i32
                }
            }
        }
    };

    // ppoll's sigmask (args[3]): the caller's mask REPLACES the blocked
    // set for the duration of the wait and the original is restored on
    // every exit — reference do_sys_poll + set_restore_sigmask. The swap
    // is what makes the mask+wait atomic: a signal queued while blocked
    // under the CALLER's mask (e.g. SIGINT blocked, then sent) is pending
    // when the wait starts with the replacement mask; the poll loop's
    // signal_pending() check now sees it unblocked and returns EINTR
    // instead of sleeping through (LTP ppoll01 SEND_SIGINT_RACE_TEST
    // slept out its 2 s timeout and returned 0; the reference returns
    // EINTR because recalc_sigpending runs after the swap).
    let sigmask_ptr = args[3] as *const u64;
    if !sigmask_ptr.is_null() {
        if !crate::arch::uaccess::access_ok(sigmask_ptr as usize, 8) {
            return -(errno::EFAULT as i64);
        }
        let new_mask = match unsafe { crate::arch::uaccess::get_user(sigmask_ptr) } {
            Some(v) => v,
            None => return -(errno::EFAULT as i64),
        };
        let scoped = match WaitScopedMask::install(new_mask) {
            Some(g) => g,
            None => return -(errno::EPERM as i64),
        };
        // Delegate to sys_poll with converted timeout; the mask is
        // restored on return by the guard (deferred to the signal path
        // when the wait ends in EINTR — see WaitScopedMask).
        let poll_args: super::SyscallArgs =
            [args[0], args[1], timeout_ms as u64, 0, 0, 0];
        let ret = sys_poll(poll_args);
        if ret == -(errno::EINTR as i64) {
            scoped.defer_restore();
        }
        return ret;
    }

    // Delegate to sys_poll with converted timeout
    let poll_args: super::SyscallArgs = [args[0], args[1], timeout_ms as u64, 0, 0, 0];
    sys_poll(poll_args)
}

/// sys_pselect6 - I/O multiplexing (pselect6 style)
///
/// # Arguments
/// - args[0]: nfds - highest file descriptor number to check + 1
/// - args[1]: readfds - pointer to readable file descriptor set
/// - args[2]: writefds - pointer to writable file descriptor set
/// - args[3]: exceptfds - pointer to exception file descriptor set
/// - args[4]: timeout - pointer to TimeSpec structure (sec, nsec)
/// - args[5]: sig - pointer to { const sigset_t *ss; size_t ss_len; }
///   (the raw-syscall shape; glibc packs the pair). A non-NULL ss
///   replaces the blocked set for the duration of the wait.
///
/// # Returns
/// Returns number of ready file descriptors on success, 0 on timeout, negative error code on failure
pub fn sys_pselect6(args: SyscallArgs) -> i64 {
    // pselect6's sixth argument is a pointer to the {ss, size} pair, not
    // the mask itself (reference SYSCALL_DEFINE6(pselect6) copies the
    // pair first, then do_sys_select validates size and copies the mask).
    let sig_pair_ptr = args[5] as *const u64;
    let mut wait_mask: Option<u64> = None;
    if !sig_pair_ptr.is_null() {
        if !crate::arch::uaccess::access_ok(sig_pair_ptr as usize, 16) {
            return -(errno::EFAULT as i64);
        }
        // SAFETY: sig_pair_ptr validated with access_ok; get_user is the
        // exception-table copy path (SUM=0 safe).
        let ss_ptr = match unsafe { crate::arch::uaccess::get_user(sig_pair_ptr) } {
            Some(v) => v as *const u64,
            None => return -(errno::EFAULT as i64),
        };
        let ss_len = match unsafe { crate::arch::uaccess::get_user(sig_pair_ptr.add(1)) } {
            Some(v) => v as usize,
            None => return -(errno::EFAULT as i64),
        };
        if !ss_ptr.is_null() {
            if ss_len != 8 {
                return -(errno::EINVAL as i64);
            }
            if !crate::arch::uaccess::access_ok(ss_ptr as usize, 8) {
                return -(errno::EFAULT as i64);
            }
            wait_mask = match unsafe { crate::arch::uaccess::get_user(ss_ptr) } {
                Some(v) => Some(v),
                None => return -(errno::EFAULT as i64),
            };
        }
    }

    // pselect6's timeout is a *timespec*, not a timeval: parsing it as a
    // timeval inflated every wait 1000x (tv_nsec read as tv_usec).
    let timeout_ptr = args[4] as *const i64; // { tv_sec, tv_nsec }
    if !timeout_ptr.is_null()
        && !crate::arch::uaccess::access_ok(timeout_ptr as usize, 16)
    {
        return -(errno::EFAULT as i64);
    }
    let (timeout_ns, has_timeout) = if timeout_ptr.is_null() {
        (0i64, false)
    } else {
        // SAFETY: timeout_ptr validated with access_ok; get_user is the
        // exception-table copy path (SUM=0 safe). An unreadable page must
        // be EFAULT, not a silent {0,0} timeout (reference core_sys_select
        // fails the copy_from_user with EFAULT).
        unsafe {
            let tv_sec = match crate::arch::uaccess::get_user(timeout_ptr) {
                Some(v) => v,
                None => return -(errno::EFAULT as i64),
            };
            let tv_nsec = match crate::arch::uaccess::get_user(timeout_ptr.add(1)) {
                Some(v) => v,
                None => return -(errno::EFAULT as i64),
            };
            if tv_sec < 0 || tv_nsec < 0 || tv_nsec >= 1_000_000_000 {
                return -(errno::EINVAL as i64);
            }
            (
                tv_sec
                    .saturating_mul(1_000_000_000)
                    .saturating_add(tv_nsec),
                true,
            )
        }
    };

    // A wait mask from the {ss, size} pair swaps the blocked set for the
    // duration of the wait, restored on every exit (reference
    // do_sys_select + set_restore_sigmask — same atomicity story as
    // ppoll's sigmask above).
    let scoped = match wait_mask {
        Some(m) => match WaitScopedMask::install(m) {
            Some(g) => Some(g),
            None => return -(errno::EPERM as i64),
        },
        None => None,
    };
    let ret = pselect6_common(args, timeout_ns, has_timeout);
    if ret == -(errno::EINTR as i64) {
        if let Some(g) = scoped {
            g.defer_restore();
        }
    }
    ret
}

/// sys_select — same core, but the timeout argument is a *timeval*
/// (tv_sec/tv_usec). It delegates to the pselect6 core with the timeout
/// pre-converted to nanoseconds.
pub fn sys_select(args: SyscallArgs) -> i64 {
    let timeout_ptr = args[4] as *const TimeVal;
    if !timeout_ptr.is_null()
        && !crate::arch::uaccess::access_ok(
            timeout_ptr as usize,
            core::mem::size_of::<TimeVal>(),
        )
    {
        return -(errno::EFAULT as i64);
    }
    let (timeout_ns, has_timeout) = if timeout_ptr.is_null() {
        (0i64, false)
    } else {
        // SAFETY: timeout_ptr validated with access_ok above; the
        // exception-table copy replaces the old raw `*timeout_ptr` deref
        // (kernel pfault on unreadable user pointers — LTP select03
        // "Faulty timeout").
        unsafe {
            let mut tv = TimeVal {
                tv_sec: 0,
                tv_usec: 0,
            };
            if crate::arch::uaccess::copy_from_user(
                &mut tv as *mut TimeVal as *mut u8,
                timeout_ptr as *const u8,
                core::mem::size_of::<TimeVal>(),
            ) != 0
            {
                return -(errno::EFAULT as i64);
            }
            if tv.tv_sec < 0 || tv.tv_usec < 0 || tv.tv_usec >= 1_000_000 {
                return -(errno::EINVAL as i64);
            }
            (
                tv.tv_sec
                    .saturating_mul(1_000_000_000)
                    .saturating_add(tv.tv_usec.saturating_mul(1000)),
                true,
            )
        }
    };

    // select has no sigmask argument; clear args[5] before entering the core.
    let core_args: SyscallArgs = [args[0], args[1], args[2], args[3], args[4], 0];
    pselect6_common(core_args, timeout_ns, has_timeout)
}

/// Core fd-set multiplexing loop shared by select(2) and pselect6(2).
/// `timeout_ns`/`has_timeout` are pre-parsed by the ABI-specific wrappers.
fn pselect6_common(args: SyscallArgs, timeout_ns: i64, has_timeout: bool) -> i64 {
    use poll_events::*;

    let nfds = args[0] as i32;
    let readfds_ptr = args[1] as *mut FdSet;
    let writefds_ptr = args[2] as *mut FdSet;
    let exceptfds_ptr = args[3] as *mut FdSet;
    let _sigmask_ptr = args[5] as *const u64;

    // Validate nfds range
    if nfds < 0 || nfds > FD_SETSIZE {
        return -(errno::EINVAL as i64);
    }

    // Check pointer validity
    let fdset_size = core::mem::size_of::<FdSet>();
    if !readfds_ptr.is_null() && !crate::arch::uaccess::access_ok(readfds_ptr as usize, fdset_size) {
        return -(errno::EFAULT as i64);
    }
    if !writefds_ptr.is_null() && !crate::arch::uaccess::access_ok(writefds_ptr as usize, fdset_size) {
        return -(errno::EFAULT as i64);
    }
    if !exceptfds_ptr.is_null() && !crate::arch::uaccess::access_ok(exceptfds_ptr as usize, fdset_size) {
        return -(errno::EFAULT as i64);
    }

    // Linux semantics: all three fd-set pointers may be NULL — with a
    // timeout that is the classic pure-sleep idiom (glibc/python sleep
    // paths; pselect6(0, NULL, NULL, NULL, &ts, NULL) returns 0 after
    // the timeout). Rejecting it with EFAULT broke python's time.sleep
    // deterministically (OSError: [Errno 14] on every call) and any
    // other sleeper that reaches select/pselect6 with no fds.
    // Original_readfds/writefds/exceptfds below are empty FdSets in
    // that case, so the loop just sleeps for the timeout / until a
    // signal, exactly like Linux.

    // Snapshot original fd_sets through the exception-table copy path
    // (SUM=0 safe; raw derefs of the user fd_sets fault the kernel).
    // SAFETY: fd set pointers validated with access_ok; reads are within FdSet size.
    let original_readfds = unsafe {
        if readfds_ptr.is_null() { FdSet::new() } else {
            let mut v = FdSet::new();
            // A faulted copy must fail the call with EFAULT, not silently
            // present an all-zero fd_set (LTP select03 "Faulty readfds").
            if crate::arch::uaccess::copy_from_user(
                &mut v as *mut FdSet as *mut u8,
                readfds_ptr as *const u8,
                fdset_size,
            ) != 0 {
                return -(errno::EFAULT as i64);
            }
            v
        }
    };
    // SAFETY: same as above.
    let original_writefds = unsafe {
        if writefds_ptr.is_null() { FdSet::new() } else {
            let mut v = FdSet::new();
            if crate::arch::uaccess::copy_from_user(
                &mut v as *mut FdSet as *mut u8,
                writefds_ptr as *const u8,
                fdset_size,
            ) != 0 {
                return -(errno::EFAULT as i64);
            }
            v
        }
    };
    // SAFETY: same as above.
    let original_exceptfds = unsafe {
        if exceptfds_ptr.is_null() { FdSet::new() } else {
            let mut v = FdSet::new();
            if crate::arch::uaccess::copy_from_user(
                &mut v as *mut FdSet as *mut u8,
                exceptfds_ptr as *const u8,
                fdset_size,
            ) != 0 {
                return -(errno::EFAULT as i64);
            }
            v
        }
    };

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(errno::EBADF as i64),
    };

    // Busy-wait loop with timeout (matching sys_poll pattern). Expiry is
    // gated on the high-res ns clock; jiffies only schedule re-checks.
    let deadline_ns = if has_timeout { poll_deadline_ns(timeout_ns) } else { None };

    loop {
        let mut result_readfds = FdSet::new();
        let mut result_writefds = FdSet::new();
        let mut result_exceptfds = FdSet::new();
        let mut ready_count = 0usize;

        for fd in 0..nfds {
            // An fd number that is set in one of the masks but has no open
            // file is EBADF (Linux select semantics — LTP select03). Fds
            // not named by any mask are simply skipped.
            let file = match fdtable.get_file(fd as usize) {
                Some(f) => f,
                None => {
                    if original_readfds.is_set(fd)
                        || original_writefds.is_set(fd)
                        || original_exceptfds.is_set(fd)
                    {
                        return -(errno::EBADF as i64);
                    }
                    continue;
                }
            };

            // Map select events to poll events
            let mut poll_events: u16 = 0;
            if original_readfds.is_set(fd) {
                poll_events |= POLLIN;
            }
            if original_writefds.is_set(fd) {
                poll_events |= POLLOUT;
            }
            if original_exceptfds.is_set(fd) {
                poll_events |= POLLERR;
            }

            if poll_events == 0 {
                continue;
            }

            // Call file's poll callback (central dispatch, D-6)
            let revents = file.poll_dispatch(poll_events);

            // Map poll revents back to select fd_sets
            if revents != 0 {
                if (revents & (POLLIN | POLLRDNORM | POLLHUP | POLLERR)) != 0
                    && original_readfds.is_set(fd)
                {
                    result_readfds.set(fd);
                    ready_count += 1;
                }
                if (revents & (POLLOUT | POLLWRNORM | POLLERR)) != 0
                    && original_writefds.is_set(fd)
                {
                    result_writefds.set(fd);
                    ready_count += 1;
                }
                if (revents & (POLLERR | POLLHUP)) != 0
                    && original_exceptfds.is_set(fd)
                {
                    result_exceptfds.set(fd);
                    ready_count += 1;
                }
            }
        }

        if ready_count > 0 {
            // SAFETY: fd set pointers validated with access_ok above;
            // copy_to_user is the exception-table copy path.
            unsafe {
                if !readfds_ptr.is_null() {
                    crate::arch::uaccess::copy_to_user(
                        readfds_ptr as *mut u8,
                        &result_readfds as *const FdSet as *const u8,
                        fdset_size,
                    );
                }
                if !writefds_ptr.is_null() {
                    crate::arch::uaccess::copy_to_user(
                        writefds_ptr as *mut u8,
                        &result_writefds as *const FdSet as *const u8,
                        fdset_size,
                    );
                }
                if !exceptfds_ptr.is_null() {
                    crate::arch::uaccess::copy_to_user(
                        exceptfds_ptr as *mut u8,
                        &result_exceptfds as *const FdSet as *const u8,
                        fdset_size,
                    );
                }
            }
            return ready_count as i64;
        }

        // No fd ready — check timeout
        if has_timeout && timeout_ns == 0 {
            // SAFETY: fd set pointers validated with access_ok above;
            // copy_to_user is the exception-table copy path.
            unsafe {
                if !readfds_ptr.is_null() {
                    crate::arch::uaccess::copy_to_user(
                        readfds_ptr as *mut u8,
                        &result_readfds as *const FdSet as *const u8,
                        fdset_size,
                    );
                }
                if !writefds_ptr.is_null() {
                    crate::arch::uaccess::copy_to_user(
                        writefds_ptr as *mut u8,
                        &result_writefds as *const FdSet as *const u8,
                        fdset_size,
                    );
                }
                if !exceptfds_ptr.is_null() {
                    crate::arch::uaccess::copy_to_user(
                        exceptfds_ptr as *mut u8,
                        &result_exceptfds as *const FdSet as *const u8,
                        fdset_size,
                    );
                }
            }
            return 0;
        }

        if has_timeout && timeout_ns > 0 {
            let expired = deadline_ns
                .map(|dl| crate::drivers::timer::monotonic_ns_since_boot() >= dl)
                .unwrap_or(true);
            if expired {
                // SAFETY: fd set pointers validated with access_ok above;
                // copy_to_user is the exception-table copy path.
                unsafe {
                    if !readfds_ptr.is_null() {
                        crate::arch::uaccess::copy_to_user(
                            readfds_ptr as *mut u8,
                            &result_readfds as *const FdSet as *const u8,
                            fdset_size,
                        );
                    }
                    if !writefds_ptr.is_null() {
                        crate::arch::uaccess::copy_to_user(
                            writefds_ptr as *mut u8,
                            &result_writefds as *const FdSet as *const u8,
                            fdset_size,
                        );
                    }
                    if !exceptfds_ptr.is_null() {
                        crate::arch::uaccess::copy_to_user(
                            exceptfds_ptr as *mut u8,
                            &result_exceptfds as *const FdSet as *const u8,
                            fdset_size,
                        );
                    }
                }
                return 0;
            }
        }

        // Check for pending signals
        if crate::signal::signal_pending() {
            return -(errno::EINTR as i64);
        }

        // P1 (busy-yield elimination): block for one re-check slice
        // instead of yield_cpu()-ing — see sys_poll / poll_sleep_slice.
        poll_sleep_slice(deadline_ns);
    }
}

/// sys_epoll_create - Create epoll instance
///
/// # Arguments
/// - args[0]: size - hint for number of events to allocate (deprecated)
///
/// # Returns
/// Returns epoll file descriptor on success, negative error code on failure
pub fn sys_epoll_create(args: SyscallArgs) -> i64 {
    let _size = args[0] as i32;

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(errno::EBADF as i64),
    };

    let epoll = alloc::boxed::Box::new(EpollFile {
        entries: crate::sync::spinlock::Spinlock::new(EpollEntryList {
            head: crate::list::ListHead::new(),
        }),
        wait_queue: crate::process::wait::WaitQueueHead::new(),
        owner_pid: crate::process::current_pid(),
        wait_count: 0,
        notify_pins: core::sync::atomic::AtomicUsize::new(0),
    });
    let epoll_ptr = alloc::boxed::Box::into_raw(epoll) as *mut u8;

    let file = alloc::sync::Arc::new(crate::fs::File::new(
        crate::fs::FileFlags::new(crate::fs::FileFlags::O_RDWR)
    ));
    file.set_ops(&EPOLL_OPS);
    file.set_private_data(epoll_ptr);

    let epoll_fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            // SAFETY: epoll_ptr was created via Box::into_raw above; reclaim to free.
            unsafe {
                let _ = alloc::boxed::Box::from_raw(epoll_ptr as *mut EpollFile);
            }
            return -(errno::EMFILE as i64);
        }
    };

    match fdtable.install_fd(epoll_fd, file) {
        Ok(()) => {
            EPOLL_INSTANCES.fetch_add(1, core::sync::atomic::Ordering::Release);
            epoll_fd as i64
        }
        Err(()) => {
            // SAFETY: epoll_ptr was created via Box::into_raw above; reclaim to free.
            unsafe {
                let _ = alloc::boxed::Box::from_raw(epoll_ptr as *mut EpollFile);
            }
            -(errno::ENOMEM as i64)
        }
    }
}

/// sys_epoll_create1 - Create epoll instance (with flags)
///
/// # Arguments
/// - args[0]: flags - flag bits
///
/// # Returns
/// Returns epoll file descriptor on success, negative error code on failure
pub fn sys_epoll_create1(args: SyscallArgs) -> i64 {
    // R32 (NEW-2): honor EPOLL_CLOEXEC instead of ignoring all flags —
    // the old delegation leaked epoll fds across execve (eventfd2 and
    // timerfd_create already support their CLOEXEC bits via the same
    // per-descriptor flag). Same install-then-flag pattern as those.
    let flags = args[0] as i32;
    const EPOLL_CLOEXEC: i32 = 0x80000;
    if flags & !EPOLL_CLOEXEC != 0 {
        return -(errno::EINVAL as i64);
    }
    let ret = sys_epoll_create([0, 0, 0, 0, 0, 0]);
    if ret >= 0 && flags & EPOLL_CLOEXEC != 0 {
        crate::fs::set_cloexec_fd(ret as usize, true);
    }
    ret
}


/// Does the epoll at `root` (transitively) watch the epoll at `needle`?
/// Members are resolved through the caller's fdtable (fd + file_id + ops
/// identity), so closed members simply drop out of the walk. `depth`
/// bounds the recursion (mirrors Linux's EP_MAX_NESTS walk bound).
fn epoll_watches_epoll(
    root: *mut EpollFile,
    needle: *mut EpollFile,
    fdtable: &crate::fs::file::FdTable,
    depth: u32,
) -> bool {
    if depth >= 6 {
        return false;
    }
    // SAFETY: caller verified root/needle via ops identity (Box::into_raw
    // EpollFile pointers, alive while their Files exist — the fdtable Arc
    // pins every member we resolve below for the duration of the walk).
    let root_ref = unsafe { &*root };
    // Cold path (nested-epoll ADD only, LTP epoll_ctl04/05 — never in OH
    // boots): collect the member list under the entries lock into a
    // BOUNDED stack array, release, then walk — the fdtable lookups and
    // the recursion stay OUT of the lock, and nothing inside the lock
    // section touches the allocator (the old Vec collect allocated under
    // the lock; a wedge there freezes every other thread on this epoll).
    // Overflow (>MAX_NEST_MEMBERS interests on ONE nested instance while
    // cycling is being checked) answers TRUE — the conservative side: the
    // ADD is refused with ELOOP instead of letting a possibly-cyclic
    // membership through unexamined.
    const MAX_NEST_MEMBERS: usize = 256;
    let mut members: [(i32, u64); MAX_NEST_MEMBERS] = [(0, 0); MAX_NEST_MEMBERS];
    let mut n_members = 0usize;
    let mut overflow = false;
    {
        let mut entries = root_ref.entries.lock();
        let head = entries.init_once();
        // SAFETY: read-only walk of nodes linked under this lock.
        let mut cur = unsafe { (*head).next };
        while cur != head {
            // SAFETY: cur is a linked EpollEntry's `list` (first field).
            let e = unsafe { &*(cur as *mut EpollEntry) };
            cur = unsafe { (*cur).next };
            if n_members < MAX_NEST_MEMBERS {
                members[n_members] = (e.fd, e.file_id);
                n_members += 1;
            } else {
                overflow = true;
                break;
            }
        }
    }
    if overflow {
        return true; // conservative: refuse the possibly-cyclic ADD
    }
    for &(fd, file_id) in members[..n_members].iter() {
        if let Some(f) = fdtable.get_file(fd as usize) {
            if f.file_id == file_id
                && f.get_ops()
                    .is_some_and(|o| core::ptr::eq(o, &EPOLL_OPS as *const _))
            {
                // SAFETY: ops identity confirmed an epoll File.
                if let Some(p) = unsafe { *f.private_data.get() } {
                    let ptr = p as *mut EpollFile;
                    if ptr == needle || epoll_watches_epoll(ptr, needle, fdtable, depth + 1) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Longest chain of nested epoll instances starting at `root` (root = 1).
fn epoll_chain_depth(
    root: *mut EpollFile,
    fdtable: &crate::fs::file::FdTable,
    depth: u32,
) -> u32 {
    if depth >= 6 {
        return 1;
    }
    // SAFETY: same ops-identity discipline as epoll_watches_epoll.
    let root_ref = unsafe { &*root };
    // Cold path — same bounded collect-then-walk shape as
    // epoll_watches_epoll (no allocation under the entries lock). Overflow
    // answers the nesting bound (6) — the conservative side: the ADD is
    // refused with EINVAL instead of under-counting a possibly-deep
    // chain.
    const MAX_NEST_MEMBERS: usize = 256;
    let mut members: [(i32, u64); MAX_NEST_MEMBERS] = [(0, 0); MAX_NEST_MEMBERS];
    let mut n_members = 0usize;
    let mut overflow = false;
    {
        let mut entries = root_ref.entries.lock();
        let head = entries.init_once();
        // SAFETY: read-only walk of nodes linked under this lock.
        let mut cur = unsafe { (*head).next };
        while cur != head {
            // SAFETY: cur is a linked EpollEntry's `list` (first field).
            let e = unsafe { &*(cur as *mut EpollEntry) };
            cur = unsafe { (*cur).next };
            if n_members < MAX_NEST_MEMBERS {
                members[n_members] = (e.fd, e.file_id);
                n_members += 1;
            } else {
                overflow = true;
                break;
            }
        }
    }
    if overflow {
        return 6; // conservative: the nesting limit refuses the ADD
    }
    let mut best = 1;
    for &(fd, file_id) in members[..n_members].iter() {
        if let Some(f) = fdtable.get_file(fd as usize) {
            if f.file_id == file_id
                && f.get_ops()
                    .is_some_and(|o| core::ptr::eq(o, &EPOLL_OPS as *const _))
            {
                // SAFETY: ops identity confirmed an epoll File.
                if let Some(p) = unsafe { *f.private_data.get() } {
                    let d = 1 + epoll_chain_depth(p as *mut EpollFile, fdtable, depth + 1);
                    best = best.max(d);
                }
            }
        }
    }
    best
}

/// sys_epoll_ctl - Control epoll instance
///
/// # Arguments
/// - args[0]: epfd - epoll file descriptor
/// - args[1]: op - operation type (ADD/DEL/MOD)
/// - args[2]: fd - target file descriptor
/// - args[3]: event - pointer to event structure
///
/// # Returns
/// Returns 0 on success, negative error code on failure
pub fn sys_epoll_ctl(args: SyscallArgs) -> i64 {
    // 120th input hunt: evdev-backed fds entering/leaving an epoll — the
    // datum that says whether the input framework's reader is armed.
    {
        static EC_N: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        let n = EC_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 400 && crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::InputTrace) {
            let fd = args[1] as usize;
            let op = args[2] as i32;
            let fid = crate::sched::get_current_fdtable()
                .and_then(|t| t.get_file(fd))
                .map(|f| f.file_id)
                .unwrap_or(0);
            let is_input = crate::drivers::input::evdev::file_id_is_input(fid);
            if is_input {
                crate::pr_warn!(
                    "IN-EPCTL #{} pid={} op={} fd={} fid={:#x}",
                    n,
                    crate::process::current_pid(),
                    op,
                    fd,
                    fid
                );
            }
        }
    }
    use epoll_ctl_ops::*;

    let epfd = args[0] as i32;
    let op = args[1] as i32;
    let fd = args[2] as i32;
    let event_ptr = args[3] as *const EPollEvent;

    if epfd < 0 || fd < 0 {
        return -(errno::EBADF as i64);
    }
    if op != EPOLL_CTL_ADD && op != EPOLL_CTL_DEL && op != EPOLL_CTL_MOD {
        return -(errno::EINVAL as i64);
    }
    if (op == EPOLL_CTL_ADD || op == EPOLL_CTL_MOD) && event_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !event_ptr.is_null() && !crate::arch::uaccess::access_ok(event_ptr as usize, core::mem::size_of::<EPollEvent>()) {
        return -(errno::EFAULT as i64);
    }

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(errno::EBADF as i64),
    };

    let ep_file = match fdtable.get_file(epfd as usize) {
        Some(f) => f,
        None => return -(errno::EBADF as i64),
    };

    // Verify the fd really is an epoll instance before treating its
    // private_data as *mut EpollFile — a regular file's private_data would
    // otherwise be type-confused into a kernel pointer.
    if !ep_file.get_ops().is_some_and(|o| core::ptr::eq(o, &EPOLL_OPS as *const _)) {
        return -(errno::EINVAL as i64);
    }

    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent mutable access.
    let epoll_ptr = match unsafe { *ep_file.private_data.get() } {
        Some(ptr) => ptr as *mut EpollFile,
        None => return -(errno::EBADF as i64),
    };
    // SAFETY: epoll_ptr came from Box::into_raw in sys_epoll_create; valid and unique.
    let epoll = unsafe { &mut *epoll_ptr };

    match op {
        EPOLL_CTL_ADD => {
            // Linux: adding an epoll instance to itself is EINVAL
            // (LTP epoll_ctl02 case 5).
            if fd == epfd {
                return -(errno::EINVAL as i64);
            }
            // R32-B9: Linux requires the target fd to be OPEN at ADD time
            // (EBADF otherwise), and the registration binds to that open
            // file description. Record the description's identity so that a
            // later close(fd)+reopen reusing the same NUMBER can never be
            // mistaken for the registered file at wait time. R36-B1: the
            // identity is the File's monotonic generation id, NOT the Arc's
            // heap address — the slab reuses a freed File's address for the
            // next same-size allocation almost deterministically, so an
            // address-keyed entry could silently match a NEW file that
            // recycled both the fd number and the address. The Arc clone
            // itself is dropped here — the entry deliberately does NOT pin
            // the file, so the last close(fd) still runs the file's close
            // op (pipe EOF etc.).
            let file = match fdtable.get_file(fd as usize) {
                Some(f) => f,
                None => return -(errno::EBADF as i64),
            };
            // Linux only accepts targets with a poll implementation —
            // directories (and other non-pollable files) get EPERM
            // (LTP epoll_ctl02 case 2 passes an open(".",
            // O_RDONLY|O_DIRECTORY) fd).
            let is_epoll = file
                .get_ops()
                .is_some_and(|o| core::ptr::eq(o, &EPOLL_OPS as *const _));
            if !is_epoll {
                // SAFETY: inode written once at open; read-only access.
                let is_dir = unsafe { &*file.inode.get() }
                    .as_ref()
                    .is_some_and(|i| i.mode.is_directory());
                let has_poll = file.get_ops().is_some_and(|o| o.poll.is_some());
                if is_dir || !has_poll {
                    return -(errno::EPERM as i64);
                }
            }
            // Nested-epoll limits (Linux EP_MAX_NESTS / ep_loop_check):
            // an epoll chain of depth 5 cannot be added anywhere (EINVAL —
            // LTP epoll_ctl04) and an ADD that would create a cycle of
            // epoll instances fails with ELOOP (LTP epoll_ctl05).
            if is_epoll {
                // SAFETY: ops identity confirmed an epoll File;
                // private_data holds the Box::into_raw EpollFile.
                let target_ptr = match unsafe { *file.private_data.get() } {
                    Some(p) => p as *mut EpollFile,
                    None => return -(errno::EBADF as i64),
                };
                if target_ptr == epoll_ptr {
                    return -(errno::EINVAL as i64);
                }
                // Cycle FIRST (a cyclic chain is also deep — Linux checks
                // ep_loop_check before the nesting limit, LTP epoll_ctl05).
                if epoll_watches_epoll(target_ptr, epoll_ptr, &fdtable, 0) {
                    return -(errno::ELOOP as i64);
                }
                // Nesting: chain depth of the target (itself counted as 1).
                if epoll_chain_depth(target_ptr, &fdtable, 0) >= 5 {
                    return -(errno::EINVAL as i64);
                }
            }
            let file_id = file.file_id;
            // SAFETY: event_ptr validated with access_ok above; copy_from_user
            // is the exception-table copy path (SUM=0 safe).
            let mut event = EPollEvent { events: 0, data: 0 };
            unsafe {
                crate::arch::uaccess::copy_from_user(
                    &mut event as *mut EPollEvent as *mut u8,
                    event_ptr as *const u8,
                    core::mem::size_of::<EPollEvent>(),
                );
            }
            // 117th (allocator-under-entries-lock hunt): the entry node is
            // Box-allocated BEFORE the entries lock is taken and freed
            // AFTER it is released on every path that does not link it —
            // nothing inside the lock section can touch the allocator.
            // The old `Vec::push` allocated on capacity growth under the
            // lock (see the EpollEntry doc for the measured stall); this
            // was the terminal wedge of the failing OH boots.
            // SAFETY: node is uniquely owned here; it is linked only under
            // the entries lock below (or freed outside it, still uniquely
            // owned).
            let node = alloc::boxed::Box::into_raw(alloc::boxed::Box::new(EpollEntry {
                list: crate::list::ListHead::new(),
                fd,
                file_id,
                events: event.events,
                data: event.data,
                last_reported: 0,
                oneshot_fired: false,
                reports: 0,
            }));
            let mut linked = false;
            enum Reg {
                Add(u64),
                Rebind(u64, u64),
                Exists,
            }
            let reg = {
                let mut entries = epoll.entries.lock();
                let head = entries.init_once();
                // SAFETY: nodes are linked and unlinked only under this
                // entries lock.
                let mut cur = unsafe { (*head).next };
                let mut existing: *mut EpollEntry = core::ptr::null_mut();
                while cur != head {
                    // SAFETY: cur is a linked EpollEntry's `list` field
                    // (first field — the cast IS the container_of).
                    let e = unsafe { &mut *(cur as *mut EpollEntry) };
                    if e.fd == fd {
                        existing = e;
                        break;
                    }
                    cur = unsafe { (*cur).next };
                }
                if existing.is_null() {
                    // The link itself is two pointer writes under the lock.
                    // SAFETY: node's list is initialized (fresh) and
                    // unlinked; head is this epoll's sentinel, held under
                    // the lock.
                    unsafe { (*node).list.add_tail(head); }
                    linked = true;
                    Reg::Add(file_id)
                } else {
                    // SAFETY: existing was derived from a linked node
                    // under this lock.
                    let existing = unsafe { &mut *existing };
                    if existing.file_id == file_id {
                        Reg::Exists
                    } else {
                        // The fd number was closed and reused between the
                        // old ADD and this one: the old open file
                        // description is gone from this fd — rebind the
                        // entry to the new one instead of returning EEXIST
                        // for a file that is not actually registered.
                        let old_id = existing.file_id;
                        existing.file_id = file_id;
                        existing.events = event.events;
                        existing.data = event.data;
                        existing.last_reported = 0;
                        existing.oneshot_fired = false;
                        // 79th (ohw124): a rebind rewrites an
                        // fd-number-keyed entry onto a NEW description —
                        // if the old description is still open elsewhere
                        // this is bookkeeping drift the dark-child hunt
                        // needs named.
                        if crate::dfx::switches::enabled(
                            crate::dfx::switches::DfxSwitch::Pid1Spin,
                        ) {
                            static REB_N: core::sync::atomic::AtomicUsize =
                                core::sync::atomic::AtomicUsize::new(0);
                            let n = REB_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                            if n < 300 {
                                crate::pr_warn!(
                                    "EPREBIND #{} pid={} epfd={} fd={} old_fid={:#x} new_fid={:#x}",
                                    n,
                                    crate::process::current_pid(),
                                    epfd,
                                    fd,
                                    old_id,
                                    file_id
                                );
                            }
                        }
                        Reg::Rebind(old_id, file_id)
                    }
                }
            };
            // EEXIST and Rebind both leave the fresh node unlinked: reclaim
            // it OUTSIDE the lock (the free can hit the allocator's slow
            // path, which must never run under the entries lock).
            // SAFETY: never linked; still uniquely owned.
            if !linked {
                unsafe { drop(alloc::boxed::Box::from_raw(node)); }
            }
            if matches!(reg, Reg::Exists) {
                return -(errno::EEXIST as i64);
            }
            // Registry helpers are called WITHOUT holding `entries` — the
            // lock order is always registry → entries (see epoll_notify_file).
            match reg {
                Reg::Add(id) => epoll_wake_register(id, epoll_ptr),
                Reg::Rebind(old, new) => {
                    epoll_wake_unregister(old, epoll_ptr);
                    epoll_wake_register(new, epoll_ptr);
                }
                Reg::Exists => unreachable!(),
            }
            // rux:epoll_ctl_insert — the interest is live (a Rebind is an
            // insert onto the reused fd number; the mask reported is the
            // new registration's).
            if crate::trace::EVENT_EPOLL_CTL_INSERT.armed() {
                crate::trace::trace_epoll_ctl_insert(
                    crate::process::current_pid() as i32,
                    epfd,
                    fd,
                    event.events,
                );
            }
        }
        EPOLL_CTL_DEL => {
            // Intrusive-list unlink (117th): find by fd, unlink under the
            // lock (two pointer writes — the old Vec::remove memmoved the
            // tail), free the node OUTSIDE the lock.
            let mut dead: core::option::Option<*mut EpollEntry> = None;
            let mut removed_id: u64 = 0;
            {
                let mut entries = epoll.entries.lock();
                let head = entries.init_once();
                // SAFETY: nodes are linked and unlinked only under this
                // entries lock.
                let mut cur = unsafe { (*head).next };
                while cur != head {
                    let next = unsafe { (*cur).next };
                    // SAFETY: cur is a linked EpollEntry's `list` (first
                    // field).
                    let e = unsafe { &mut *(cur as *mut EpollEntry) };
                    if e.fd == fd {
                        // SAFETY: e.list is linked in this epoll's list.
                        unsafe { e.list.del(); }
                        removed_id = e.file_id;
                        dead = Some(e as *mut EpollEntry);
                        break;
                    }
                    cur = next;
                }
            }
            if let Some(node) = dead {
                // 79th (ohw124): init's paramservice child fd 21 lost its
                // epoll interest between two snapshots with no purge print
                // — attribute EVERY interest removal (pid, fd, file_id,
                // whether the fd still resolves to the same description:
                // a DEL of a still-live registered fd is the dark-child
                // signature; a foreign-process DEL names the diverging
                // caller directly). dfx=pid1spin-gated.
                if crate::dfx::switches::enabled(
                    crate::dfx::switches::DfxSwitch::Pid1Spin,
                ) {
                    static DEL_N: core::sync::atomic::AtomicUsize =
                        core::sync::atomic::AtomicUsize::new(0);
                    let n = DEL_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    if n < 300 {
                        let live = crate::sched::get_current_fdtable()
                            .and_then(|ft| ft.get_file(fd as usize))
                            .map(|f| f.file_id)
                            .unwrap_or(0);
                        crate::pr_warn!(
                            "EPDEL #{} pid={} epfd={} fd={} fid={:#x} fd_now={:#x}",
                            n,
                            crate::process::current_pid(),
                            epfd,
                            fd,
                            removed_id,
                            live
                        );
                    }
                }
                epoll_wake_unregister(removed_id, epoll_ptr);
                // rux:epoll_ctl_remove — the interest is gone.
                if crate::trace::EVENT_EPOLL_CTL_REMOVE.armed() {
                    crate::trace::trace_epoll_ctl_remove(
                        crate::process::current_pid() as i32,
                        epfd,
                        fd,
                    );
                }
                // Free the unlinked node with NO entries lock held (117th
                // allocation discipline).
                // SAFETY: unlinked above under the lock; nodes are
                // reachable only through the epoll's entries list, and
                // interest nodes are never shared.
                unsafe { drop(alloc::boxed::Box::from_raw(node)); }
            } else {
                return -(errno::ENOENT as i64);
            }
        }
        EPOLL_CTL_MOD => {
            // SAFETY: event_ptr validated with access_ok above; copy_from_user
            // is the exception-table copy path (SUM=0 safe).
            let mut event = EPollEvent { events: 0, data: 0 };
            unsafe {
                crate::arch::uaccess::copy_from_user(
                    &mut event as *mut EPollEvent as *mut u8,
                    event_ptr as *const u8,
                    core::mem::size_of::<EPollEvent>(),
                );
            }
            // Intrusive-list walk (117th): mutate in place, no allocation.
            let mut found = false;
            {
                let mut entries = epoll.entries.lock();
                let head = entries.init_once();
                // SAFETY: nodes are linked and unlinked only under this
                // entries lock.
                let mut cur = unsafe { (*head).next };
                while cur != head {
                    // SAFETY: cur is a linked EpollEntry's `list` (first
                    // field).
                    let entry = unsafe { &mut *(cur as *mut EpollEntry) };
                    cur = unsafe { (*cur).next };
                    if entry.fd == fd {
                        entry.events = event.events;
                        entry.data = event.data;
                        // Re-arming via MOD resets the edge-triggered
                        // snapshot (Linux: MOD re-injects the fd into the
                        // ready list) and re-arms a fired EPOLLONESHOT
                        // registration.
                        entry.last_reported = 0;
                        entry.oneshot_fired = false;
                        found = true;
                        break;
                    }
                }
            }
            if !found {
                return -(errno::ENOENT as i64);
            }
        }
        _ => return -(errno::EINVAL as i64),
    }

    0
}

/// dfx=pid1spin (69th): dump pid `pid`'s epoll interests with the live
/// readiness of each watched file — the discriminator between the three
/// ways init's LoopEvent can go dark over the param children:
///   (a) the interest is GONE (purged as a dead fd / never added),
///   (b) the interest mask is EPOLLOUT-only (the reference LoopEvent's
///       LE_Send flips it there until the reply flushes — if EPOLLOUT
///       is never delivered the child is blind to incoming data),
///   (c) the interest has EPOLLIN but the scan still misses the queued
///       data (a poll/readiness divergence).
/// For unix-socket targets each line carries the connection pair state
/// (own queue segs/bytes, EOF/dead, peer truesize vs sndbuf cap). For
/// other files the line names the ops table identity. Diagnostic-only
/// cross-task fdtable walk (same contract as the taskdump path).
/// 118th param-stall conviction instrument (timed at ~300 s by the tick
/// hook in trap.rs): the ladder-stall verdict reduced to "init handles
/// zero param-set messages while samgr publishes readiness into a void".
/// This dump discriminates the three seed hypotheses directly:
///   H1 (EPOLLET edge loss on the param unix socket): a unix socket fd of
///      pid 1 with PENDING queue data whose epoll interest row shows an
///      ET entry with last_reported already set to the current readiness
///      (edge swallowed), or the socket missing from every interest set.
///   H2 (always-ready fd spinning init's loop): an interest row whose
///      file polls permanently ready without queue data.
///   H3 (sender-side userspace wedge): every unix socket of pid 1 (and
///      its peers) EMPTY — no data ever arrived; blame moves to the image.
/// Prints: pid 1's fd inventory (epoll / unix / pipe / other), every unix
/// socket's queue depth + peer state, the full epoll interest dump
/// (fd/events/fid/ET snapshot/reports per row), and the trace-ring tail.
pub fn param_stall_dump() {
    let task = unsafe { crate::sched::find_task_by_pid(1) };
    if task.is_null() {
        return;
    }
    // 120th (loglevel hunt): the direct question — WHERE is init's main
    // thread parked and what does its stack say? Full tgid-1 dump with
    // kernel + user stack frames (VMA/file-mapped) via the RWFRZ
    // forensics path (ConsoleWriter = direct UART, release-visible).
    let dumped = crate::sync::futex::rwfz_dump_tgid(1, 8, true);
    crate::pr_warn!("PARAM-STALL tgid1-stackdump tasks={}", dumped);
    // 122nd (sandboxed-app hang): dump the live launcher/appspawn
    // process — the app cycle spawns, sandboxes, pivots, then hangs
    // silently; this names where its threads park.
    {
        let mut launcher_tgid: u32 = 0;
        let mut appspawn_tgid: u32 = 0;
        crate::sched::for_each_task_try(|task| {
            let tk = unsafe { &*task };
            let cb: &[u8; 16] = tk.comm();
            let cl = cb.iter().position(|&b| b == 0).unwrap_or(16);
            let comm = core::str::from_utf8(&cb[..cl]).unwrap_or("");
            if launcher_tgid == 0 && comm.contains("auncher") {
                launcher_tgid = tk.tgid() as u32;
            }
            if appspawn_tgid == 0 && comm.contains("ppspawn") {
                appspawn_tgid = tk.tgid() as u32;
            }
        });
        if launcher_tgid != 0 {
            let n = crate::sync::futex::rwfz_dump_tgid(launcher_tgid, 10, true);
            crate::pr_warn!("APP-DUMP launcher tgid={} tasks={}", launcher_tgid, n);
        } else {
            crate::pr_warn!("APP-DUMP launcher: no live launcher task at dump");
        }
        if appspawn_tgid != 0 {
            let n = crate::sync::futex::rwfz_dump_tgid(appspawn_tgid, 6, true);
            crate::pr_warn!("APP-DUMP appspawn tgid={} tasks={}", appspawn_tgid, n);
        }
        // hilogd — the reader that drains the sockets apps writev to; if
        // it is dead or its reader is starved, every app's logger jams.
        {
            let mut hilog_tgid: u32 = 0;
            crate::sched::for_each_task_try(|task| {
                let tk = unsafe { &*task };
                let cb: &[u8; 16] = tk.comm();
                let cl = cb.iter().position(|&b| b == 0).unwrap_or(16);
                let comm = core::str::from_utf8(&cb[..cl]).unwrap_or("");
                if hilog_tgid == 0 && comm.contains("hilog") {
                    hilog_tgid = tk.tgid() as u32;
                }
            });
            if hilog_tgid != 0 {
                let n = crate::sync::futex::rwfz_dump_tgid(hilog_tgid, 10, true);
                crate::pr_warn!("APP-DUMP hilogd tgid={} tasks={}", hilog_tgid, n);
            } else {
                crate::pr_warn!("APP-DUMP hilogd: NO LIVE TASK");
            }
        }
        // Full fd tables for hilogd and the live launcher: the busy pair is
        // writev(fd=3) on one side, recvmsg(fd=9) on the other — name every
        // fd's identity so the jammed pair is confirmed and its counters
        // (segs/bytes on both ends) read directly.
        for (label, needle) in [("hilogd", "hilog"), ("launcher", "auncher")] {
            let mut tgid: u32 = 0;
            crate::sched::for_each_task_try(|task| {
                let tk = unsafe { &*task };
                let cb: &[u8; 16] = tk.comm();
                let cl = cb.iter().position(|&b| b == 0).unwrap_or(16);
                let comm = core::str::from_utf8(&cb[..cl]).unwrap_or("");
                if tgid == 0 && comm.contains(needle) {
                    tgid = tk.tgid() as u32;
                }
            });
            if tgid == 0 {
                continue;
            }
            let mut leader: *mut crate::process::task::Task = core::ptr::null_mut();
            crate::sched::for_each_task_try(|task| {
                let tk = unsafe { &*task };
                if leader.is_null() && tk.tgid() as u32 == tgid && tk.pid() as u32 == tgid {
                    leader = task;
                }
            });
            if leader.is_null() {
                continue;
            }
            // SAFETY: read-only diagnostic fd walk of the found leader.
            let tk = unsafe { &*leader };
            // Collect rows under the walk (diag takes recv_queue inside
            // the fdtable walk's callback context; printing there also
            // nests console locks) — print AFTER, from a plain Vec.
            let mut rows: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
            let _ = tk.try_fdtable().map(|fdt| {
                fdt.for_each_file(|fd, f| {
                    if fd > 15 {
                        return;
                    }
                    let ops_addr = f.get_ops().map(|o| o as *const _ as usize).unwrap_or(0);
                    let unix_line = crate::net::unix::unix_of_file_diag(f)
                        .map(|d| {
                            let (segs, qb, eof, dead, peer_alive, peer_ts, sndcap, acceptq, self_ptr, peer_raw, charge_v, err_v) =
                                d.diag_stream_pair();
                            alloc::format!(
                                " unix[segs={} bytes={} eof={} dead={} peer_alive={} peer_ts={} sndcap={} accq={} self={:#x} charge={} err={} peerW={:#x}]",
                                segs, qb, eof as u8, dead as u8, peer_alive as u8, peer_ts, sndcap, acceptq, self_ptr, peer_raw, charge_v, err_v
                            )
                        })
                        .unwrap_or_default();
                    rows.push(alloc::format!(
                        "APP-FD {} tgid={} fd={} ops={:#x}{}",
                        label, tgid, fd, ops_addr, unix_line
                    ));
                })
            });
            for r in rows.iter() {
                crate::pr_warn!("{}", r);
            }
        }
    }
    // …and the last 48 syscalls of pid 1 — the userspace loop's actual
    // behavior at the stall (needs dfx=sysforensic to be recording).
    crate::dfx::taskdump::dump_syscall_ring_for(1);
    // SAFETY: read-only diagnostic walk of pid 1's fdtable.
    let tk = unsafe { &*task };
    let fdtable = match tk.try_fdtable_mut() {
        Some(f) => f,
        None => return,
    };
    // ---- fd inventory + per-socket queue depths (incl. fds NOT in any
    // epoll interest set — H1's dead-fd face) ----
    let mut n_total = 0usize;
    let mut n_epoll = 0usize;
    let mut unix_rows: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
    fdtable.for_each_file(|fd, f| {
        n_total += 1;
        let ops = f.get_ops().map(|o| o as *const _ as usize).unwrap_or(0);
        if core::ptr::eq(f.get_ops().unwrap(), &EPOLL_OPS as *const _) {
            n_epoll += 1;
        }
        // Unix sockets: queue depth + peer liveness (diag contract —
        // read-only, own locks).
        if let Some(d) = crate::net::unix::unix_of_file_diag(f) {
            let (segs, qb, eof, dead, peer_alive, peer_ts, sndcap, acceptq, _sp, _pr, _ch, _ev) =
                d.diag_stream_pair();
            if segs > 0 || eof || acceptq > 0 {
                unix_rows.push(alloc::format!(
                    "  unix fd={} segs={} bytes={} eof={} dead={} peer_alive={} peer_ts={} sndcap={} acceptq={}",
                    fd, segs, qb, eof as u8, dead as u8, peer_alive as u8, peer_ts, sndcap, acceptq
                ));
            }
        }
    });
    crate::pr_warn!(
        "PARAM-STALL pid1 fds={} epoll={} unix-with-data-or-eof={} ops_self={:#x}",
        n_total,
        n_epoll,
        unix_rows.len(),
        &EPOLL_OPS as *const _ as usize
    );
    for r in unix_rows.iter().take(24) {
        crate::pr_warn!("PARAM-STALL {}", r);
    }
    // ---- full epoll interest dump (fd/events/fid/ET snapshot/reports) ----
    pid1_epoll_interest_dump(1);
    // ---- trace-ring tail (newest records; EPOLL_CTL/DELIVERY/WAIT_SCAN
    // rows for pid 1 name the last scheduler-visible epoll activity) ----
    let snap = crate::trace::render_snapshot();
    let tail: alloc::string::String = snap
        .chars()
        .rev()
        .take(2400)
        .collect::<alloc::vec::Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    crate::pr_warn!("PARAM-STALL trace-tail begin");
    // 119th conviction: the verification print the first capture lacked —
    // dump the WW_WATCH slot table itself (slot, task ptr, pid/comm, and
    // the per-task mini-ring fill index) so an empty PARAM-INIT-WW ring is
    // PROVABLY "watched but zero events" vs "never watched" (the 118th's
    // watch-add was guarded by current().is_none() which never fires in
    // tick context — see trap.rs WW-P1-WATCH-ADD).
    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::SchedTrace) {
        let mut p1_watched = false;
        for (i, w) in crate::sched::sched::WW_WATCH.iter().enumerate() {
            let t = w.load(core::sync::atomic::Ordering::Relaxed);
            if t == 0 {
                continue;
            }
            // SAFETY: read-only pid/comm read of a watched task; the slot
            // was pinned by ww_watch_add and tasks on the watch list are
            // never freed past a note_task_freed barrier (sched.rs).
            let (pid, comm) = unsafe { ((*(t as *mut crate::process::task::Task)).pid(), (*(t as *mut crate::process::task::Task)).comm()) };
            let cl = comm.iter().position(|&b| b == 0).unwrap_or(16);
            let comm_str = core::str::from_utf8(&comm[..cl]).unwrap_or("?");
            let n0 = crate::sched::sched::ww_tail_fill(i);
            if pid == 1 {
                p1_watched = true;
            }
            crate::pr_warn!(
                "WW-WATCH-SLOT #{} task={:#x} pid={} ({}) ring_events={}",
                i,
                t,
                pid,
                comm_str,
                n0
            );
        }
        if !p1_watched {
            crate::pr_warn!("WW-WATCH-SLOT pid 1 NOT on the watch list (watch-add failed?)");
        }
    }
    crate::pr_warn!("{}", tail);
    crate::pr_warn!("PARAM-STALL trace-tail end");
    // 118th: pid 1's own threads' parked syscalls (the LASTSYS latch is
    // armed from the first tick on hunt boots — see dispatch.rs), plus
    // fd-table file identities for fd 0-8 of the main thread. The ladder
    // break is "which syscall is init's main thread parked in" — this
    // answers it directly at 300 s.
    unsafe {
        crate::process::pid_hash::pid_hash_for_each_task_try(|t| {
            let tk = &*t;
            if tk.tgid() != 1 {
                return;
            }
            let nr = tk.last_syscall_nr.load(core::sync::atomic::Ordering::Relaxed);
            let a0 = tk.last_syscall_a0.load(core::sync::atomic::Ordering::Relaxed);
            let a1 = tk.last_syscall_a1.load(core::sync::atomic::Ordering::Relaxed);
            let cb: &[u8; 16] = tk.comm();
            let cl = cb.iter().position(|&b| b == 0).unwrap_or(16);
            let comm = core::str::from_utf8(&cb[..cl]).unwrap_or("?");
            crate::pr_warn!(
                "PARAM-INIT pid={} ({}) st={:#x} pk={} lastsys={}(a0={:#x},a1={:#x})",
                tk.pid(),
                comm,
                tk.state().bits(),
                tk.pick_state(),
                nr,
                a0,
                a1
            );
            // 118th: dump this thread's wakewatch mini-ring — for pid 1's
            // main thread this names which scheduler stage last touched
            // it (W wake-ok / L link / P pick-unlink / K publish /
            // A affirm) while it sits in the never-returning pwait.
            let mut wws = alloc::string::String::new();
            crate::sched::sched::ww_append_dump(
                &mut wws,
                t,
                crate::drivers::timer::get_jiffies(),
            );
            if !wws.is_empty() {
                crate::pr_warn!("PARAM-INIT-WW{}", wws);
            }
        });
    }
    param_sender_dump();
}

/// 118th sender-naming pass (second half of the param-stall conviction):
/// walk EVERY task for unix sockets whose PEER is a socket owned by pid 1
/// — the would-be senders into init's param pipeline — plus every
/// param/beget-family comm. Each row carries the sender's scheduler
/// wake/pick state (pk mark, claim, on_cpu, CFS linkage, on_rq via the
/// entity, preempt count, parked-syscall latches) so an orphan-marked or
/// wake-refused sender is visible on the line itself.
pub fn param_sender_dump() {
    use crate::process::task::{SchedPolicy, Task, TaskState};
    // init's unix socket identities (Arc raw pointers), one fdtable pass.
    let init_task = unsafe { crate::sched::find_task_by_pid(1) };
    if init_task.is_null() {
        return;
    }
    let mut init_socks: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
    if let Some(ft) = unsafe { &*init_task }.try_fdtable() {
        ft.for_each_file(|_fd, f| {
            if let Some(s) = crate::net::unix::unix_of_file_diag(f) {
                init_socks.push(alloc::sync::Arc::into_raw(s) as usize);
            }
        });
    }
    let mut rows = 0usize;
    crate::pr_warn!(
        "PARAM-SENDERS init_socks={} — tasks connected to init + param/beget comms:",
        init_socks.len()
    );
    // SAFETY: read-only diagnostic iteration (pid_hash try contract).
    unsafe {
        crate::process::pid_hash::pid_hash_for_each_task_try(|t| {
            if rows >= 40 {
                return;
            }
            let tk = &*t;
            let pid = tk.pid();
            if pid == 0 {
                return;
            }
            let comm = tk.comm();
            let comm_len = comm.iter().position(|&b| b == 0).unwrap_or(16);
            let comm_s = core::str::from_utf8(&comm[..comm_len]).unwrap_or("?");
            let family = comm_s.contains("param") || comm_s.contains("beget") || comm_s.contains("watcher");
            // connected-to-init scan
            let mut conn: Option<(usize, usize, usize)> = None; // (fd, own_segs, own_bytes)
            if !init_socks.is_empty() {
                if let Some(ft) = tk.try_fdtable() {
                    ft.for_each_file(|fd, f| {
                        if conn.is_some() {
                            return;
                        }
                        if let Some(s) = crate::net::unix::unix_of_file_diag(f) {
                            if let Some(peer) = crate::net::unix::unix_peer_of_file(f) {
                                let p = alloc::sync::Arc::into_raw(peer) as usize;
                                if init_socks.contains(&p) {
                                    let (segs, qb, ..) = s.diag_stream_pair();
                                    conn = Some((fd, segs, qb));
                                }
                            }
                        }
                    });
                }
            }
            if !family && conn.is_none() {
                return;
            }
            rows += 1;
            let linked = match crate::sched::sched::grq_diag_cfs_linked(t) {
                Some(true) => "1",
                Some(false) => "0",
                None => "?",
            };
            let onrq = match tk.policy() {
                SchedPolicy::Fifo | SchedPolicy::Rr => tk.rt_entity().is_on_rq(),
                SchedPolicy::Deadline => tk.dl_entity().is_on_rq(),
                _ => tk.sched_entity().is_on_rq(),
            };
            let nr = tk.last_syscall_nr.load(core::sync::atomic::Ordering::Relaxed);
            let a0 = tk.last_syscall_a0.load(core::sync::atomic::Ordering::Relaxed);
            crate::pr_warn!(
                "PARAM-SENDER pid={} ({}) st={:#x} preempt={} pk={} claim={} on_cpu={} linked={} on_rq={} conn={} lastsys={}(a0={:#x})",
                pid,
                comm_s,
                tk.state().bits(),
                tk.preempt_count(),
                tk.pick_state(),
                tk.running_on_cpu.load(core::sync::atomic::Ordering::Acquire),
                tk.on_cpu(),
                linked,
                onrq,
                match conn {
                    Some((fd, s, b)) => alloc::format!("fd={}:segs={}:bytes={}", fd, s, b),
                    None => alloc::string::String::from("none-but-family"),
                },
                nr,
                a0
            );
        });
    }
    crate::pr_warn!("PARAM-SENDERS end rows={}", rows);
}

pub fn pid1_epoll_interest_dump(pid: crate::process::task::Pid) {
    // SAFETY: diagnostic lookup of the target task (find_task_by_pid
    // contract); read-only walks of its fdtable below.
    let task = unsafe { crate::sched::find_task_by_pid(pid) };
    if task.is_null() {
        return;
    }
    // SAFETY: task is a live task pointer from the scheduler registry.
    let tk = unsafe { &*task };
    let fdtable = match tk.try_fdtable_mut() {
        Some(f) => f,
        None => return,
    };
    let mut epfds: alloc::vec::Vec<(usize, alloc::sync::Arc<crate::fs::File>)> =
        alloc::vec::Vec::new();
    fdtable.for_each_file(|fd, f| {
        if f.get_ops()
            .is_some_and(|o| core::ptr::eq(o, &EPOLL_OPS as *const _))
        {
            epfds.push((fd, alloc::sync::Arc::clone(f)));
        }
    });
    if epfds.is_empty() {
        crate::pr_warn!("PID1EPOLL pid={} no epoll instances", pid);
        return;
    }
    for (epfd, ep_file) in epfds.iter() {
        // SAFETY: ops identity confirmed an epoll File; private_data
        // holds the Box::into_raw EpollFile.
        let epoll_ptr = match unsafe { *ep_file.private_data.get() } {
            Some(p) => p as *mut EpollFile,
            None => continue,
        };
        // SAFETY: epoll_ptr came from Box::into_raw in sys_epoll_create;
        // valid while the fdtable holds this Arc (cloned above).
        let epoll = unsafe { &*epoll_ptr };
        let mut entries = epoll.entries.lock();
        // Intrusive-list walk (117th): count first for the header, then
        // the per-entry rows (diagnostic path, dfx=pid1spin-gated).
        let head = entries.init_once();
        // SAFETY: read-only counting walk of nodes linked under this lock.
        let mut total: usize = 0;
        {
            let mut cur = unsafe { (*head).next };
            while cur != head {
                total += 1;
                cur = unsafe { (*cur).next };
            }
        }
        crate::pr_warn!(
            "PID1EPOLL pid={} epfd={} entries={} wait_count={}",
            pid,
            epfd,
            total,
            epoll.wait_count
        );
        // fd numbers covered by an interest (for the fdtable diff below).
        let mut watched_fds: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
        // SAFETY: read-only walk of linked EpollEntry nodes; nodes are
        // unlinked only under this same lock (the loop body only reads).
        let mut cur = unsafe { (*head).next };
        while cur != head {
            // SAFETY: cur is a linked EpollEntry's `list` (first field).
            let entry = unsafe { &*(cur as *mut EpollEntry) };
            cur = unsafe { (*cur).next };
            watched_fds.push(entry.fd as usize);
            let target = fdtable.get_file(entry.fd as usize);
            let (fid_ok, unix_line, ops_id, ops_tag) = match &target {
                Some(f) => {
                    let fid_ok = f.file_id == entry.file_id;
                    let ops_id = f
                        .get_ops()
                        .map(|o| o as *const _ as usize)
                        .unwrap_or(0);
                    // 118th ladder hunt: name the fd TYPE for the
                    // known OPS statics — the loop's timerfd/eventfd/
                    // signalfd members are the job-timer suspects.
                    let ops_tag: &str = f
                        .get_ops()
                        .map(|o| {
                            if core::ptr::eq(o, &TIMERFD_OPS as *const _) {
                                " timerfd"
                            } else if core::ptr::eq(o, &EVENTFD_OPS as *const _) {
                                " eventfd"
                            } else if core::ptr::eq(o, &SIGNALFD_OPS as *const _) {
                                " signalfd"
                            } else if core::ptr::eq(o, &EPOLL_OPS as *const _) {
                                " epoll"
                            } else if core::ptr::eq(o, &crate::fs::pipe::PIPE_OPS as *const _) {
                                " pipe"
                            } else {
                                ""
                            }
                        })
                        .unwrap_or("");
                    let unix_line = crate::net::unix::unix_of_file_diag(f)
                        .map(|s| {
                            let (segs, qb, eof, dead, peer_alive, peer_ts, sndcap, acceptq, _sp, _pr, _ch, _ev) =
                                s.diag_stream_pair();
                            alloc::format!(
                                " unix[segs={} bytes={} eof={} dead={} peer_alive={} peer_ts={} sndcap={} wr={} accq={}]",
                                segs,
                                qb,
                                eof as u8,
                                dead as u8,
                                peer_alive as u8,
                                peer_ts,
                                sndcap,
                                (peer_alive && peer_ts < sndcap) as u8,
                                acceptq
                            )
                        })
                        .unwrap_or_default();
                    (fid_ok, unix_line, ops_id, ops_tag)
                }
                None => (false, alloc::string::String::new(), 0, ""),
            };
            crate::pr_warn!(
                "PID1EPOLL  fd={} mask={:#x} et={} oneshot={} reports={} file_id_match={} ops={:#x}{}{}{}{}",
                entry.fd,
                entry.events,
                (entry.events & epoll_events::EPOLLET != 0) as u8,
                (entry.events & epoll_events::EPOLLONESHOT != 0) as u8,
                entry.reports,
                fid_ok as u8,
                ops_id,
                ops_tag,
                if target.is_none() { " FD-GONE" } else { "" },
                unix_line,
                // 120th: the fd's path — names the listener (paramservice
                // class) vs the client sockets, the datum that identifies
                // which registered fd never saw a connection arrive.
                {
                    let t1 = unsafe { crate::sched::find_task_by_pid(1) };
                    let tk1 = if t1.is_null() { None } else { Some(unsafe { &*t1 }) };
                    let p = tk1
                        .and_then(|tk| {
                            tk.try_fdtable().and_then(|fdt| {
                                fdt.get_file(entry.fd as usize).map(|file| file.path())
                            })
                        })
                        .unwrap_or_default();
                    p
                }
            );
        }
        drop(entries);
        // 69th addendum (the ohw82 second face): a paramservice child
        // that is IN the fdtable but has NO epoll interest is invisible
        // to the dump above — yet its queue is where client sends land
        // (ohw82: PCTL-SEND q=3/2276 while every interest showed
        // segs=0). Diff the fdtable against the interest set: every
        // unix-socket fd with an EPOLLIN-less life gets a line here,
        // with the peer-identity that would explain a stale link.
        fdtable.for_each_file(|fd, f| {
            if watched_fds.contains(&fd) {
                return;
            }
            if let Some(s) = crate::net::unix::unix_of_file_diag(f) {
                let (segs, qb, eof, dead, peer_alive, peer_ts, sndcap, acceptq, _sp, _pr, _ch, _ev) =
                    s.diag_stream_pair();
                // Only sockets that look connected-and-live matter for
                // the diff; listeners show up too (accq>0 is itself a
                // finding — a deaf accept).
                crate::pr_warn!(
                    "PID1EPOLL  UNWATCHED fd={} fid={:#x} unix[segs={} bytes={} eof={} dead={} peer_alive={} peer_ts={} sndcap={} wr={} accq={}]",
                    fd,
                    f.file_id,
                    segs,
                    qb,
                    eof as u8,
                    dead as u8,
                    peer_alive as u8,
                    peer_ts,
                    sndcap,
                    (peer_alive && peer_ts < sndcap) as u8,
                    acceptq
                );
            }
        });
    }
}

/// sys_epoll_wait - Wait for epoll events
///
/// # Arguments
/// - args[0]: epfd - epoll file descriptor
/// - args[1]: events - pointer to event array
/// - args[2]: maxevents - maximum number of events
/// - args[3]: timeout - timeout in milliseconds
///
/// # Returns
/// Returns number of ready events on success, 0 on timeout, negative error code on failure
pub fn sys_epoll_wait(args: SyscallArgs) -> i64 {
    // rux:epoll_wait_exit — the syscall's verdict in one record (the
    // EINTR-storm / timeout classes the epolltrace hunt needed). The
    // wrapper sees every return path of the inner body.
    let ret = sys_epoll_wait_inner(args);
    if crate::trace::EVENT_EPOLL_WAIT_EXIT.armed() {
        crate::trace::trace_epoll_wait_exit(
            crate::process::current_pid() as i32,
            args[0] as i32,
            ret,
            args[3] as i32,
        );
    }
    ret
}

fn sys_epoll_wait_inner(args: SyscallArgs) -> i64 {
    use epoll_events::*;
    use poll_events::*;

    let epfd = args[0] as i32;
    let events_ptr = args[1] as *mut EPollEvent;
    let maxevents = args[2] as i32;
    let timeout_ms = args[3] as i32;

    // Linux ordering: a negative/closed epfd is EBADF even when the
    // other arguments are also bad (LTP epoll_wait03 case 1 passes -1).
    if epfd < 0 {
        return -(errno::EBADF as i64);
    }
    if events_ptr.is_null() || maxevents <= 0 {
        return -(errno::EINVAL as i64);
    }

    let events_size = core::mem::size_of::<EPollEvent>() * (maxevents as usize);
    if !crate::arch::uaccess::access_ok(events_ptr as usize, events_size) {
        return -(errno::EFAULT as i64);
    }

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(errno::EBADF as i64),
    };

    let ep_file = match fdtable.get_file(epfd as usize) {
        Some(f) => f,
        None => return -(errno::EBADF as i64),
    };

    // Verify the fd really is an epoll instance before treating its
    // private_data as *mut EpollFile (type-confusion guard).
    if !ep_file.get_ops().is_some_and(|o| core::ptr::eq(o, &EPOLL_OPS as *const _)) {
        return -(errno::EINVAL as i64);
    }

    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent mutable access.
    let epoll_ptr = match unsafe { *ep_file.private_data.get() } {
        Some(ptr) => ptr as *mut EpollFile,
        None => return -(errno::EBADF as i64),
    };
    // SAFETY: epoll_ptr came from Box::into_raw in sys_epoll_create; valid and unique.
    let epoll = unsafe { &mut *epoll_ptr };

    // High-resolution expiry gate (see poll_deadline_ns); jiffies only
    // schedule the intermediate re-check sleeps.
    let deadline_ns = poll_deadline_ns(timeout_ms as i64 * 1_000_000);

        // Allocation discipline (s2-a review finding P2, hardened by the
        // 117th intrusive-list port): every scratch buffer used INSIDE the
        // entries-locked scan is hoisted out here and pre-sized, so the
        // lock section is allocation-free on the ungated hot path (with
        // tracing off and no dead interests, nothing below can touch the
        // allocator while the lock is held). The reference has NO small
        // maxevents cap (EP_MAX_EVENTS = INT_MAX/sizeof); readiness is
        // bounded by the INTEREST COUNT, not maxevents, so the capacity
        // is clamped for allocation safety and the Vec grows if a huge
        // interest set ever exceeds it.
        let alloc_cap = (maxevents as usize).min(4096);
        let mut ready_events: alloc::vec::Vec<EPollEvent> =
            alloc::vec::Vec::with_capacity(alloc_cap);
        // dfx=epolltrace: identity of each ready fd (fd, file ops table) so
        // the trace line can be symbolized against the kernel ELF. Pushed
        // only while tracing is armed (diagnostic boots accept the alloc).
        let mut trace_fds: alloc::vec::Vec<(i32, u32, *const ())> =
            alloc::vec::Vec::with_capacity(32);
        // Unlinked dead-interest nodes (117th): the scan unlinks dead
        // entries as it walks (the old code collected fd numbers and ran
        // Vec::retain after the loop); the node frees run OUTSIDE the
        // lock. Capped: a scan round with more than 128 dead interests
        // leaves the excess linked — they are re-detected (and unlinked)
        // by the next round, so removal stays correct, just deferred.
        let mut dead_nodes: alloc::vec::Vec<*mut EpollEntry> =
            alloc::vec::Vec::with_capacity(128);
        // 119th (epoll-pwait conviction): the per-round readiness probe
        // for init's main thread (pid 1). Stack buffers — no allocation
        // under the entries lock.
        const PROBE_CAP: usize = 48;
        let mut probe_fds: [i32; PROBE_CAP] = [0; PROBE_CAP];
        let mut probe_rev: [u16; PROBE_CAP] = [0; PROBE_CAP];
        let probe_n = core::sync::atomic::AtomicUsize::new(0);
        let probe_armed = crate::process::current_pid() == 1;
        // P3 wiring set (see the event-driven wait block below): hoisted
        // so the per-round build never allocates under the entries lock.
        // EPOLL_WIRE_CAP bounds it — an interest set larger than the cap
        // falls back to the 10ms re-check slice instead of growing the
        // Vec inside the lock.
        const EPOLL_WIRE_CAP: usize = 512;
        let mut watched: alloc::vec::Vec<(
            alloc::sync::Arc<crate::fs::File>,
            u16,
            core::option::Option<u32>,
        )> = alloc::vec::Vec::with_capacity(EPOLL_WIRE_CAP);
        loop {
        let mut entries = epoll.entries.lock();
        ready_events.clear();
        trace_fds.clear();
        dead_nodes.clear();
        // Dead interests found during the scan (fd number no longer
        // resolves to the registered description). Linux removes an
        // epoll interest at close(2) (eventpoll_release); the wait path
        // must never manufacture events for one. The old behavior
        // reported EPOLLERR|EPOLLHUP on EVERY wait forever — a permanent
        // phantom wakeup that spun GLib main loops. sys_close purges the
        // common case; this catches anything that slips past (fd-number
        // reuse without EPOLL_CTL_DEL).
        let mut nent: usize = 0;
        // Intrusive-list walk (117th): `next` is captured before the body
        // so the dead-interest arm can unlink in place. Nodes are linked
        // and unlinked only under this entries lock.
        // SAFETY: every node reachable from the sentinel is linked under
        // this lock.
        let head = entries.init_once();
        let mut cur = unsafe { (*head).next };
        while cur != head {
            let next = unsafe { (*cur).next };
            // SAFETY: cur is a linked EpollEntry's `list` field (first
            // field — the cast IS the container_of).
            let entry = unsafe { &mut *(cur as *mut EpollEntry) };
            cur = next;
            nent += 1;
            // Linux epoll_wait never consumes readiness it does not deliver:
            // events beyond `maxevents` stay on the ready list and are
            // re-reported by the next wait. Our snapshot-scan model must
            // match that — STOP the scan at maxevents so undelivered entries
            // keep their edge-trigger snapshot (`last_reported`) untouched.
            // Marking-and-dropping them silently consumed their edges: under
            // a ready burst (more sockets readable at once than maxevents)
            // the truncated EPOLLET fds never reported again — readiness
            // never went idle (the data stayed unread), so no new edge could
            // fire, and the watcher stopped reading those fds forever
            // (Xorg registers its client sockets EPOLLET: a burst of client
            // messages wedged every client past maxevents while the server
            // kept spinning on the fds it still saw).
            if ready_events.len() >= maxevents as usize {
                break;
            }
            // R32-B9: the registration binds to the open file description
            // recorded at ADD time (entry.file_id). The fd NUMBER alone is
            // not enough: after close(fd) and reuse of the number by an
            // unrelated file, the old lookup silently polled the NEW file
            // and reported its readiness with the OLD entry's user data.
            // Only accept the mapping while the fd still resolves to the
            // same description; otherwise keep the pre-existing closed-fd
            // polarity (EPOLLERR|EPOLLHUP).  R36-B1: file_id is the File's
            // generation id (never reused), so a reopened fd can never
            // re-match a stale entry even when the allocator recycles the
            // old File's heap address.
            let file = match fdtable.get_file(entry.fd as usize) {
                Some(f) if f.file_id == entry.file_id => f,
                _ => {
                    // The interest is dead (closed fd / reused number).
                    // Remove it silently — Linux delivers NOTHING for a
                    // closed interest (removed at close), and the former
                    // forever-EPOLLERR|EPOLLHUP report was a phantom
                    // wakeup source.
                    // 69th (ohw82 second face): a purged interest on a
                    // LIVE child = that child goes dark forever (client
                    // sends land in a queue nobody polls — the 105s).
                    // dfx=pid1spin names every purge for pid 1's epoll:
                    // fd-gone vs fd-reused (and the new file's class).
                    // 79th: the pid==1 restriction dropped — ohw124 lost
                    // fd 21's interest with NO purge print, so if this
                    // path fired at all it was from a non-pid-1 context.
                    if crate::dfx::switches::enabled(
                        crate::dfx::switches::DfxSwitch::Pid1Spin,
                    )
                    {
                        static PURGE_N: core::sync::atomic::AtomicUsize =
                            core::sync::atomic::AtomicUsize::new(0);
                        let n = PURGE_N
                            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                        if n < 200 {
                            let now = match fdtable.get_file(entry.fd as usize) {
                                Some(f) => alloc::format!(
                                    "reused fid={:#x} ops={:#x}",
                                    f.file_id,
                                    f.get_ops()
                                        .map(|o| o as *const _ as usize)
                                        .unwrap_or(0)
                                ),
                                None => "gone".into(),
                            };
                            crate::pr_warn!(
                                "PID1EPOLL-PURGE #{} fd={} entry_fid={:#x} fd_now={}",
                                n,
                                entry.fd,
                                entry.file_id,
                                now
                            );
                        }
                    }
                    // 117th: unlink the dead node NOW (its `next` was
                    // captured before this body — the walk stays valid)
                    // and defer the free to after the lock; past the
                    // dead_nodes cap the entry stays linked and is
                    // re-detected next round.
                    if dead_nodes.len() < dead_nodes.capacity() {
                        // SAFETY: entry.list is linked in this epoll's
                        // list (walked under this lock).
                        unsafe { entry.list.del(); }
                        dead_nodes.push(entry as *mut EpollEntry);
                    }
                    continue;
                }
            };

            // Map epoll events to poll events
            let mut poll_mask: u16 = 0;
            if entry.events & EPOLLIN != 0 { poll_mask |= POLLIN; }
            if entry.events & EPOLLOUT != 0 { poll_mask |= POLLOUT; }
            if entry.events & EPOLLRDHUP != 0 { poll_mask |= POLLRDHUP; }

            // EPOLLONESHOT: a registration that has already reported is
            // disabled until EPOLL_CTL_MOD re-arms it — report nothing,
            // not even EPOLLERR/EPOLLHUP.
            if entry.events & EPOLLONESHOT != 0 && entry.oneshot_fired {
                continue;
            }

            // Central dispatch (D-6): the capability fn or the
            // always-ready fallback (the fallback's *NORM companions map
            // to the same epoll event bits the old mask-echo did).
            let revents = file.poll_dispatch(poll_mask);

            // 119th (epoll-pwait conviction): the readiness PROBE for
            // init's main thread. The waiter is woken hundreds of
            // times per second and never sees a ready fd — the probe
            // names, per scan round, WHICH fds report readiness and
            // what the readiness bits are, so the verdict separates
            // "a ready fd the scan failed to report" (a mapping bug)
            // from "nothing is ready" (the event was consumed by the
            // sibling waiter before this scan).
            if probe_armed {
                let n = probe_n.load(core::sync::atomic::Ordering::Relaxed);
                if n < PROBE_CAP {
                    probe_fds[n] = entry.fd;
                    probe_rev[n] = revents;
                    probe_n.store(n + 1, core::sync::atomic::Ordering::Relaxed);
                }
            }

            if revents != 0 {
                let mut ep_events: u32 = 0;
                if revents & (POLLIN | POLLRDNORM | POLLHUP) != 0 { ep_events |= EPOLLIN; }
                if revents & (POLLOUT | POLLWRNORM) != 0 { ep_events |= EPOLLOUT; }
                if revents & POLLRDHUP != 0 { ep_events |= EPOLLRDHUP; }
                if revents & POLLERR != 0 { ep_events |= EPOLLERR; }
                if revents & POLLHUP != 0 { ep_events |= EPOLLHUP; }

                // EPOLLET (P2): edge-triggered registrations only report
                // on a readiness TRANSITION. The readiness snapshot is
                // the full converted event set (pre-mask): a repeated
                // wait with unchanged readiness reports nothing until
                // the fd goes fully idle (resetting the snapshot, so the
                // next arrival is a fresh edge) or a new event bit
                // appears (e.g. EPOLLOUT joins EPOLLIN).
                if entry.events & EPOLLET != 0 {
                    if ep_events == entry.last_reported {
                        continue; // same edge — already reported
                    }
                    entry.last_reported = ep_events;
                }
                // EPOLLONESHOT: this delivery disables the registration
                // until EPOLL_CTL_MOD re-arms it.
                if entry.events & EPOLLONESHOT != 0 {
                    entry.oneshot_fired = true;
                }

                // Linux always reports EPOLLERR/EPOLLHUP even when the
                // registration did not subscribe to them — they are not
                // maskable. Only the I/O readiness bits honor entry.events.
                let report_mask = entry.events | EPOLLERR | EPOLLHUP;
                if crate::dfx::switches::enabled(
                    crate::dfx::switches::DfxSwitch::EpollTrace,
                ) || crate::trace::EVENT_EPOLL_DELIVERY.armed() {
                    trace_fds.push((
                        entry.fd,
                        ep_events & report_mask,
                        file.get_ops().map_or(core::ptr::null(), |o| o as *const _ as *const ()),
                    ));
                }
                // dfx=socksnap (b16): count the delivery.
                entry.reports = entry.reports.saturating_add(1);
                ready_events.push(EPollEvent {
                    events: ep_events & report_mask,
                    data: entry.data,
                });
            } else if entry.events & EPOLLET != 0 {
                // Readiness fully drained: arm the trigger for the next
                // arrival.
                entry.last_reported = 0;
            }
        }
        drop(entries);
        // 119th (epoll-pwait conviction): the per-round readiness
        // summary for init's main thread. Printed AFTER the entries lock
        // is dropped (printk may allocate) and only for pid 1.
        if probe_armed {
            let n = probe_n.load(core::sync::atomic::Ordering::Relaxed).min(PROBE_CAP);
            use core::sync::atomic::{AtomicU32, Ordering as Rel};
            static PROBE_ROUNDS: AtomicU32 = AtomicU32::new(0);
            let r = PROBE_ROUNDS.fetch_add(1, Rel::Relaxed);
            if r < 20 || r % 500 == 0 {
                let mut s = alloc::format!("EPPROBE round={} nfd={} ready=[", r, n);
                let mut nonzero = 0usize;
                for i in 0..n {
                    if probe_rev[i] != 0 {
                        nonzero += 1;
                        s.push_str(&alloc::format!("{}:{:#x} ", probe_fds[i], probe_rev[i]));
                    }
                }
                s.push_str(&alloc::format!("] nonzero={} events={}", nonzero, ready_events.len()));
                crate::pr_warn!("{}", s);
            }
            probe_n.store(0, Rel::Relaxed);
        }
        // Free the dead-interest nodes the scan unlinked — OUTSIDE the
        // entries lock (117th allocation discipline: a free can hit the
        // allocator's slow path, which must never run under the lock).
        // SAFETY: each node was unlinked under the entries lock above;
        // nodes are reachable only through the epoll's entries list, and
        // interest nodes are never shared.
        if !dead_nodes.is_empty() {
            for node in dead_nodes.drain(..) {
                unsafe { drop(alloc::boxed::Box::from_raw(node)); }
            }
        }

        // rux:epoll_wait_scan — the scan round's census (the epollscan
        // hunt's datum: the scan RAN and what it collected). One record
        // per round; a looping wait shows the rounds.
        if crate::trace::EVENT_EPOLL_WAIT_SCAN.armed() {
            crate::trace::trace_epoll_wait_scan(
                crate::process::current_pid() as i32,
                epfd,
                nent as u32,
                ready_events.len() as u32,
            );
        }

        // dfx=epollscan (leg-9 epoll hunt): one raw-console line per scan —
        // the r13ep2 boot had 14 events drained into the device queue yet
        // every epoll_wait returned idle, so the missing datum is whether
        // the scan ran at all, how many interests it saw, and what it
        // collected. Rate-limited: first 400 then 1-in-100.
        if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::EpollScan) {
            use core::sync::atomic::{AtomicU64, Ordering};
            static SC_N: AtomicU64 = AtomicU64::new(0);
            let n = SC_N.fetch_add(1, Ordering::Relaxed);
            if n < 400 || n % 100 == 0 {
                eps_msg(b"EPSCAN[");
                eps_u64(n);
                eps_msg(b"] pid=");
                if let Some(cur) = crate::sched::current() {
                    // SAFETY: read-only pid/comm access on the current task.
                    let (pid, comm) = unsafe { ((*cur).pid(), (*cur).comm()) };
                    eps_u64(pid as u64);
                    eps_msg(b" ");
                    for &c in comm.iter() {
                        if c == 0 { break; }
                        crate::console::putchar_no_lock(c);
                    }
                }
                eps_msg(b" nent=");
                eps_u64(nent as u64);
                eps_msg(b" ready=");
                eps_u64(ready_events.len() as u64);
                eps_msg(b"\n");
            }
        }

        if !ready_events.is_empty() {
            // rux:epoll_delivery — one record per delivered interest (the
            // epolltrace hunt's fd/ops identity; the ring's overwrite
            // discipline replaces the bespoke rate limit).
            if crate::trace::EVENT_EPOLL_DELIVERY.armed() {
                let pid = crate::process::current_pid() as i32;
                for (fd, ev, ops) in trace_fds.iter() {
                    crate::trace::trace_epoll_delivery(
                        pid,
                        *fd,
                        *ev,
                        *ops as usize as u64,
                    );
                }
            }
            // dfx=epolltrace: who got woken by which fd. Rate-limited to the
            // first 400 deliveries plus every 2000th after, so a spinning
            // waiter does not flood the serial console.
            if crate::dfx::switches::enabled(
                crate::dfx::switches::DfxSwitch::EpollTrace,
            ) {
                use core::sync::atomic::{AtomicU64, Ordering};
                static N: AtomicU64 = AtomicU64::new(0);
                let n = N.fetch_add(1, Ordering::Relaxed);
                if n < 400 || n % 2000 == 0 {
                    if let Some(cur) = crate::sched::current() {
                        // SAFETY: read-only comm/pid access on the current task.
                        let (pid, comm) =
                            unsafe { ((*cur).pid(), (*cur).comm()) };
                        let comm_str = core::str::from_utf8(comm)
                            .unwrap_or("?\u{0}")
                            .trim_end_matches('\u{0}');
                        let mut line: alloc::string::String =
                            alloc::format!("EPTRC[{}] pid={} {} n={}", n, pid, comm_str, ready_events.len());
                        for (fd, ev, ops) in trace_fds.iter().take(8) {
                            line.push_str(&alloc::format!(
                                " fd={} ev={:#x} ops={:#x}",
                                fd,
                                ev,
                                *ops as usize
                            ));
                        }
                        crate::pr_info!("{}", line);
                    }
                }
            }
            let count = ready_events.len().min(maxevents as usize);
            // SAFETY: events_ptr validated with access_ok; copy_to_user is
            // the exception-table copy path (SUM=0 safe). A fault (e.g. a
            // read-only page) must fail with EFAULT, not silently succeed
            // (LTP epoll_wait03 case 2).
            let uncopied = unsafe {
                crate::arch::uaccess::copy_to_user(
                    events_ptr as *mut u8,
                    ready_events.as_ptr() as *const u8,
                    count * core::mem::size_of::<EPollEvent>(),
                )
            };
            if uncopied > 0 {
                return -(errno::EFAULT as i64);
            }
            // dfx=socksnap (b16): one completed wait syscall.
            epoll.wait_count = epoll.wait_count.saturating_add(1);
            return count as i64;
        }

        // Check timeout
        if timeout_ms == 0 {
            return 0;
        }
        if timeout_ms > 0 {
            let expired = deadline_ns
                .map(|dl| crate::drivers::timer::monotonic_ns_since_boot() >= dl)
                .unwrap_or(true);
            if expired {
                return 0;
            }
        }

        if crate::signal::signal_pending() {
            // dfx=epolltrace: an EINTR-storm burns a waiter at ~100% CPU
            // with ZERO ready deliveries (invisible to the delivery trace).
            // Rate-limited print of the pending mask so a storm is obvious.
            if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::EpollTrace)
            {
                use core::sync::atomic::{AtomicU64, Ordering};
                static N: AtomicU64 = AtomicU64::new(0);
                let n = N.fetch_add(1, Ordering::Relaxed);
                if n < 200 || n % 2000 == 0 {
                    if let Some(cur) = crate::sched::current() {
                        // SAFETY: read-only pid/comm/pending access on the
                        // current task.
                        let (pid, comm) = unsafe { ((*cur).pid(), (*cur).comm()) };
                        let comm_str = core::str::from_utf8(comm)
                            .unwrap_or("?")
                            .trim_end_matches('\u{0}');
                        crate::pr_info!(
                            "EPTRC-EINTR[{}] pid={} {} sigpnd={:#x}",
                            n,
                            pid,
                            comm_str,
                            (*cur).pending.get_all()
                        );
                    }
                }
            }
            return -(errno::EINTR as i64);
        }

        // P3 (event-driven wait): register the WAITING TASK on every watched
        // file's readiness queue (poll_wait dispatch) and sleep until a
        // producer event on any of them — no tick-rate re-check wakeups.
        // Falls back to the P1 10ms slice below when any watched fd has no
        // wake path. ET snapshot semantics are unchanged: they are driven by
        // the readiness scan (and epoll_notify_file for unix sockets), not
        // by how the waiter slept.
        {
            // 117th: `watched` is the hoisted, pre-sized buffer (see the
            // scan's scratch-buffer block) — the old per-round Vec::new()
            // allocated INSIDE the entries lock on its first push. Beyond
            // the cap the round degrades to the 10ms slice path (correct,
            // latency-degraded only for interest sets > EPOLL_WIRE_CAP).
            watched.clear();
            let mut wired = true;
            {
                let mut entries = epoll.entries.lock();
                // Intrusive-list walk (117th): read-only pass, no
                // allocation under the entries lock.
                let head = entries.init_once();
                // SAFETY: read-only walk of nodes linked under this lock.
                let mut cur = unsafe { (*head).next };
                while cur != head {
                    // SAFETY: cur is a linked EpollEntry's `list` (first
                    // field).
                    let entry = unsafe { &*(cur as *mut EpollEntry) };
                    cur = unsafe { (*cur).next };
                    match fdtable.get_file(entry.fd as usize) {
                        Some(f) if f.file_id == entry.file_id => {
                            let mut poll_mask: u16 = 0;
                            if entry.events & EPOLLIN != 0 { poll_mask |= POLLIN; }
                            if entry.events & EPOLLOUT != 0 { poll_mask |= POLLOUT; }
                            let et_last = if entry.events & EPOLLET != 0 {
                                Some(entry.last_reported)
                            } else {
                                None
                            };
                            if watched.len() >= EPOLL_WIRE_CAP {
                                // Slice fallback (same contract as a dead
                                // interest below): never grow under the
                                // lock.
                                wired = false;
                                break;
                            }
                            watched.push((f, poll_mask, et_last));
                        }
                        // Dead interest: the slice path (and the next scan's
                        // dead-fd purge) handles it.
                        _ => {
                            wired = false;
                            break;
                        }
                    }
                }
            }
            if wired && !watched.is_empty() {
                let current = match crate::sched::current() {
                    Some(t) => t,
                    None => return 0, // no task context: cannot block
                };
                // Quick readiness re-check inside the wait round. Mirrors
                // the scan's ET logic: an edge-triggered entry whose current
                // readiness equals its last-reported snapshot contributes
                // nothing (sleeping is correct — the scan would skip it).
                let quick_ready = || -> bool {
                    for (f, mask, et_last) in watched.iter() {
                        // Central dispatch (D-6): capability fn or the
                        // always-ready fallback (same epoll mapping as
                        // the old mask-echo — see the scan above).
                        let revents = f.poll_dispatch(*mask);
                        if revents == 0 {
                            continue;
                        }
                        let mut ep_events: u32 = 0;
                        if revents & (POLLIN | POLLRDNORM | POLLHUP) != 0 { ep_events |= EPOLLIN; }
                        if revents & (POLLOUT | POLLWRNORM) != 0 { ep_events |= EPOLLOUT; }
                        if revents & POLLERR != 0 { ep_events |= EPOLLERR; }
                        if revents & POLLHUP != 0 { ep_events |= EPOLLHUP; }
                        if let Some(last) = et_last {
                            if ep_events == *last {
                                continue; // same ET edge — already reported
                            }
                        }
                        return true;
                    }
                    false
                };
                match poll_event_wait_round(
                    &watched.iter().map(|(f, _, _)| alloc::sync::Arc::clone(f))
                        .collect::<alloc::vec::Vec<_>>(),
                    current,
                    deadline_ns,
                    &quick_ready,
                ) {
                    PollEventWait::Interrupted => return -(errno::EINTR as i64),
                    PollEventWait::Woken => {
                        // Drop the Arc clones before the next round (the
                        // old block-scoped Vec dropped them here too).
                        watched.clear();
                        continue;
                    }
                    PollEventWait::NotWired => {} // fall through to the slice
                }
            }
            // Drop the Arc clones before the P1 slice sleep (the old
            // block-scoped Vec dropped them at this point — the buffer
            // itself is retained for reuse).
            watched.clear();
        }

        // P1 (busy-yield elimination, fallback): block for one re-check
        // interval instead of yield_cpu()-ing around the poll loop (which
        // burned a full CPU per waiter under SMP). Wake sources:
        //   - the 10ms re-check timer armed below — readiness is re-polled
        //     on every wake, so this timer bounds event latency for fd
        //     types with no producer wake path,
        //   - signal delivery (INTERRUPTIBLE + signal_wake_up → EINTR),
        //   - epoll_notify_file via EPOLL_WAKE_REGISTRY.
        let current = match crate::sched::current() {
            Some(t) => t,
            None => return 0, // no task context: cannot block
        };
        epoll.wait_queue.prepare_to_wait(current, false, true);
        // Re-check for signals after prepare_to_wait (lost-wake discipline —
        // a signal landing between the check above and the state transition
        // must not be slept through).
        if crate::signal::signal_pending() {
            epoll.wait_queue.finish_wait(current);
            // SAFETY: current is the running task's pointer; undo a
            // concurrent wake enqueue (NEW-C2 discipline).
            unsafe { crate::sched::dequeue_task(&*current); }
            return -(errno::EINTR as i64);
        }
        // One slice = min(remaining timeout, 10ms re-check interval) —
        // and when the remaining wait fits inside one slice, the wake is
        // a HIGH-RESOLUTION timer at the precise ns deadline (grid-tick
        // rounding made a 1 ms epoll_wait timeout sleep ~10 ms; LTP
        // epoll_wait02 precision budget, median 9998us for a 1000us).
        let slice_ns: u64 = EPOLL_RECHECK_JIFFIES
            .saturating_mul(1_000_000_000 / crate::drivers::timer::HZ as u64);
        // SAFETY: current is the running task's pointer.
        let my_pid = unsafe { (*current).pid() };
        let remaining_ns = deadline_ns
            .map(|dl| dl.saturating_sub(crate::drivers::timer::monotonic_ns_since_boot()));
        let timer_id = match (deadline_ns, remaining_ns) {
            (Some(dl), Some(_rem)) if _rem <= slice_ns => {
                let id = crate::timer::add_timer_wakeup_hres(
                    jiffies_covering_deadline(dl),
                    deadline_ns_to_hres_ticks(dl),
                    my_pid,
                );
                crate::drivers::timer::rearm_for_hres();
                id
            }
            _ => crate::timer::add_timer_wakeup(
                crate::drivers::timer::get_jiffies() + EPOLL_RECHECK_JIFFIES,
                my_pid,
            ),
        };
        if timer_id == 0 {
            // Timer pool exhausted: degrade to a single yield rather than
            // sleep forever (nothing else is guaranteed to wake us).
            if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::EpollTrace)
            {
                use core::sync::atomic::{AtomicU64, Ordering};
                static N: AtomicU64 = AtomicU64::new(0);
                let n = N.fetch_add(1, Ordering::Relaxed);
                if n < 100 || n % 5000 == 0 {
                    if let Some(cur) = crate::sched::current() {
                        // SAFETY: read-only pid/comm access on the current task.
                        let (pid, comm) = unsafe { ((*cur).pid(), (*cur).comm()) };
                        let comm_str = core::str::from_utf8(comm)
                            .unwrap_or("?")
                            .trim_end_matches('\u{0}');
                        crate::pr_info!(
                            "EPTRC-YIELD[{}] pid={} {} (timer pool full)",
                            n,
                            pid,
                            comm_str
                        );
                    }
                }
            }
            epoll.wait_queue.finish_wait(current);
            // SAFETY: current is the running task's pointer.
            unsafe { crate::sched::dequeue_task(&*current); }
            crate::sched::yield_cpu();
            continue;
        }
        // Enable interrupts before schedule(): syscall context runs with
        // SIE=0; __schedule must save SIE=1 so the switched-back task has
        // interrupts enabled (same contract as wait_event!).
        crate::arch::cpu::restore_irq(true);
        crate::sched::schedule();
        epoll.wait_queue.finish_wait(current);
        // Spurious/early wake (signal before expiry): drop the timer so it
        // cannot enqueue us again while running.
        crate::timer::del_timer(timer_id);
    }
}

/// sys_epoll_pwait - Wait for epoll events (with signal mask)
///
/// # Arguments
/// - args[0]: epfd - epoll file descriptor
/// - args[1]: events - pointer to event array
/// - args[2]: maxevents - maximum number of events
/// - args[3]: timeout - timeout in milliseconds
/// - args[4]: sigmask - pointer to signal mask
///
/// # Returns
/// Returns number of ready events on success, 0 on timeout, negative error code on failure
pub fn sys_epoll_pwait(args: SyscallArgs) -> i64 {
    // 120th (loglevel hunt): pid 1's pwait park signature — the timeout
    // argument separates an infinite wait (-1) from a long-poll retry
    // (huge ms). Budgeted, first 12 then 1-in-2000.
    {
        use core::sync::atomic::{AtomicU64, Ordering as Rel};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Rel::Relaxed);
        if crate::process::current_pid() == 1 && (n < 12 || n % 2000 == 0) {
            crate::pr_warn!(
                "P1PWAIT epfd={} timeout={} events_ptr={:#x}",
                args[0],
                args[3] as i64,
                args[1]
            );
        }
    }
    // P3 LTP round 3: epoll_pwait's sigmask argument REPLACES the blocked
    // mask for the duration of the wait (Linux semantics). Validate the
    // pointer (EFAULT) and size (EINVAL), apply, wait, restore. Without
    // the temporary mask a signal that the caller asked to block
    // interrupted the wait (LTP epoll_pwait01) and a bad pointer went
    // unnoticed (epoll_pwait04).
    let sigmask_ptr = args[4] as *const u64;
    let sigsetsize = args[5] as usize;
    if sigmask_ptr.is_null() {
        return sys_epoll_wait([args[0], args[1], args[2], args[3], 0, 0]);
    }
    if sigsetsize != 8 {
        return -(errno::EINVAL as i64);
    }
    if !crate::arch::uaccess::access_ok(sigmask_ptr as usize, 8) {
        return -(errno::EFAULT as i64);
    }
    let new_mask = match unsafe { crate::arch::uaccess::get_user(sigmask_ptr) } {
        Some(v) => v,
        None => return -(errno::EFAULT as i64),
    };
    // SIGKILL/SIGSTOP are unblockable (same rule as rt_sigprocmask).
    let new_mask = new_mask & !((1u64 << 8) | (1u64 << 18));
    let current = match crate::sched::current() {
        Some(c) => c as *const _ as *mut crate::process::task::Task,
        None => return -(errno::EPERM as i64),
    };
    // SAFETY: current is the running task; sigmask is a plain u64 field.
    let saved_mask = unsafe { (*current).sigmask };
    unsafe { (*current).sigmask = new_mask; }
    let ret = sys_epoll_wait([args[0], args[1], args[2], args[3], 0, 0]);
    unsafe { (*current).sigmask = saved_mask; }
    ret
}

// ============================================================================
// eventfd
// ============================================================================

/// EFD_SEMAPHORE: read returns 1 and decrements counter by 1 (instead of returning full counter)
const EFD_SEMAPHORE: u32 = 0x1;

/// eventfd backend: 64-bit counter
struct EventFd {
    counter: core::sync::atomic::AtomicU64,
    /// EFD_SEMAPHORE flag
    semaphore: bool,
    /// Waiters blocked for counter != 0 (readers) or counter decrease (writers)
    wait_queue: crate::process::wait::WaitQueueHead,
}

impl EventFd {
    fn new(initval: u64, flags: u32) -> Self {
        Self {
            counter: core::sync::atomic::AtomicU64::new(initval),
            semaphore: (flags & EFD_SEMAPHORE) != 0,
            wait_queue: crate::process::wait::WaitQueueHead::new(),
        }
    }

    /// wait_event-style interruptible block until `ready()` holds.
    /// Returns Err(EINTR) when interrupted by a signal.
    fn block_until(&self, ready: impl Fn() -> bool) -> Result<(), i32> {
        loop {
            if ready() {
                return Ok(());
            }
            let current = match crate::sched::current() {
                Some(t) => t,
                None => return Ok(()), // cannot block: fall back to caller
            };
            // Atomically register + mark INTERRUPTIBLE, then re-check.
            self.wait_queue.prepare_to_wait(current, false, true);
            if ready() {
                self.wait_queue.finish_wait(current);
                // NEW-C2: undo a concurrent wake enqueue before looping.
                // SAFETY: current is the running task's pointer.
                unsafe { crate::sched::dequeue_task(&*current); }
                return Ok(());
            }
            if crate::signal::signal_pending() {
                self.wait_queue.finish_wait(current);
                // SAFETY: current is the running task's pointer.
                unsafe { crate::sched::dequeue_task(&*current); }
                return Err(errno::EINTR);
            }
            crate::arch::cpu::restore_irq(true);
            crate::sched::schedule();
            self.wait_queue.finish_wait(current);
            if crate::signal::signal_pending() {
                return Err(errno::EINTR);
            }
        }
    }
}

fn eventfd_read(file: &crate::fs::File, buf: &mut [u8]) -> isize {
    if buf.len() < 8 {
        return -errno::EINVAL as isize;
    }
    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return -errno::EBADF as isize,
    };
    // SAFETY: ptr came from Box::into_raw in sys_eventfd2; valid and properly aligned.
    let efd = unsafe { &*(ptr as *const EventFd) };

    // Blocking semantics (review批次1: previously returned EAGAIN even for
    // blocking fds, busy-looping every poll/read caller).
    if efd.counter.load(core::sync::atomic::Ordering::Relaxed) == 0 {
        if file.flags().bits() & crate::fs::file::FileFlags::O_NONBLOCK != 0 {
            return -errno::EAGAIN as isize;
        }
        match efd.block_until(|| efd.counter.load(core::sync::atomic::Ordering::Relaxed) != 0) {
            Ok(()) => {}
            Err(e) => return -(e) as isize,
        }
    }

    loop {
        let val = efd.counter.load(core::sync::atomic::Ordering::Relaxed);
        if val == 0 {
            if file.flags().bits() & crate::fs::file::FileFlags::O_NONBLOCK != 0 {
                return -errno::EAGAIN as isize;
            }
            // Raced to zero between the wake and the CAS: block again.
            match efd.block_until(|| efd.counter.load(core::sync::atomic::Ordering::Relaxed) != 0) {
                Ok(()) => continue,
                Err(e) => return -(e) as isize,
            }
        }
        let new_val = if efd.semaphore { val - 1 } else { 0 };
        if efd.counter.compare_exchange_weak(
            val, new_val,
            core::sync::atomic::Ordering::AcqRel,
            core::sync::atomic::Ordering::Relaxed,
        ).is_ok() {
            let return_val = if efd.semaphore { 1u64 } else { val };
            buf[..8].copy_from_slice(&return_val.to_le_bytes());
            // A reader consumed — writers waiting for space can proceed.
            efd.wait_queue.wake_up_all();
            // EPOLLET lost-edge fix (same class as the timerfd d3a94b4d
            // fix): the counter DROP is a readiness edge for EPOLLOUT
            // watchers (the eventfd gained write space) and, for
            // EFD_SEMAPHORE drains, an EPOLLIN mask change. The
            // reference wakes the file's wait queue (eventfd_read_ctx →
            // wake_up_locked_poll EPOLLOUT) which rides ep_poll_callback;
            // our registry equivalent is the notify below. Without it an
            // ET EPOLLOUT registration on a counter-full fd stays
            // suppressed forever.
            crate::syscall::misc::epoll_notify_file(file.file_id);
            return 8;
        }
        // CAS failed, retry
    }
}

fn eventfd_write(file: &crate::fs::File, buf: &[u8]) -> isize {
    if buf.len() < 8 {
        return -errno::EINVAL as isize;
    }
    let val = u64::from_le_bytes([buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7]]);
    if val == u64::MAX {
        return -errno::EINVAL as isize;
    }

    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return -errno::EBADF as isize,
    };
    // SAFETY: ptr came from Box::into_raw in sys_eventfd2; valid and properly aligned.
    let efd = unsafe { &*(ptr as *const EventFd) };

    loop {
        let cur = efd.counter.load(core::sync::atomic::Ordering::Relaxed);
        let new = cur.checked_add(val);
        match new {
            Some(n) => {
                if efd.counter.compare_exchange_weak(
                    cur, n,
                    core::sync::atomic::Ordering::AcqRel,
                    core::sync::atomic::Ordering::Relaxed,
                ).is_ok() {
                    // Counter no longer zero — wake blocked readers.
                    efd.wait_queue.wake_up_all();
                    // EPOLLET lost-edge fix (same class as the timerfd
                    // d3a94b4d fix): a write that lands while NOBODY
                    // waits must still re-arm edge-triggered interests —
                    // the ET entry's last_reported mask stayed set and
                    // every later epoll_wait suppressed the readable fd.
                    // The reference's ep_poll_callback runs from
                    // wake_up_poll on the file's wait queue
                    // unconditionally (eventfd_write →
                    // wake_up_locked_poll EPOLLIN). Probed before
                    // patching (et_fd_probe eventfd row): 1/300 edges
                    // reported vs Linux 300/300; with this notify 300/300.
                    crate::syscall::misc::epoll_notify_file(file.file_id);
                    return 8;
                }
                // CAS failed, retry
            }
            None => {
                // Overflow: counter + val > u64::MAX
                let flags = file.flags().bits();
                if flags & crate::fs::file::FileFlags::O_NONBLOCK != 0 {
                    return -errno::EAGAIN as isize;
                }
                // Blocking mode: wait for a reader to drain the counter
                // (review批次1: previously returned EAGAIN immediately).
                let cur_ok = || {
                    efd.counter.load(core::sync::atomic::Ordering::Relaxed)
                        .checked_add(val).is_some()
                };
                match efd.block_until(cur_ok) {
                    Ok(()) => continue,
                    Err(e) => return -(e) as isize,
                }
            }
        }
    }
}

fn eventfd_poll(file: &crate::fs::File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    let mut ready = 0u16;

    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return POLLERR,
    };
    // SAFETY: ptr came from Box::into_raw in sys_eventfd2; valid and properly aligned.
    let efd = unsafe { &*(ptr as *const EventFd) };

    let counter = efd.counter.load(core::sync::atomic::Ordering::Relaxed);

    if events & POLLIN != 0 && counter > 0 {
        ready |= POLLIN | POLLRDNORM;
    }
    if events & POLLOUT != 0 && counter < u64::MAX - 1 {
        ready |= POLLOUT | POLLWRNORM;
    }

    ready
}

fn eventfd_close(file: &crate::fs::File) -> i32 {
    // R20-5: free the boxed EventFd — File has no Drop impl and
    // private_data is a raw pointer, so the old comment ("freed when File
    // is dropped") was wrong: it leaked on every close. Runs only on the
    // last Arc reference, so no read/write/poll can be using it.
    // SAFETY: private_data is an UnsafeCell; we hold &File for the last
    // reference.
    if let Some(ptr) = unsafe { *file.private_data.get() } {
        // SAFETY: ptr came from Box::into_raw in sys_eventfd2.
        unsafe {
            let _ = alloc::boxed::Box::from_raw(ptr as *mut EventFd);
        }
        unsafe { *file.private_data.get() = None; }
    }
    0
}

/// EventFd file operations (D-6 builder form)
pub static EVENTFD_OPS: crate::fs::FileOps = crate::fs::FileOps::builder()
    .read(eventfd_read)
    .write(eventfd_write)
    .close(eventfd_close)
    .poll(eventfd_poll)
    .build();

// ==================== timerfd ====================

/// timerfd backend: timer + expiration counter
struct TimerFd {
    /// Clock ID (CLOCK_REALTIME=0, CLOCK_MONOTONIC=1)
    clockid: i32,
    /// Kernel timer ID (0 = disarmed)
    kernel_timer_id: u64,
    /// Interval in jiffies (0 = one-shot)
    interval_jiffies: u64,
    /// Number of timer expirations since last read()
    expiration_count: core::sync::atomic::AtomicU64,
    /// Readers blocked until the timer fires
    wait_queue: crate::process::wait::WaitQueueHead,
    /// Owning File's generation id (0 until create finishes) — the
    /// expiry path notifies the epoll registry with it (the EPOLLET
    /// lost-edge fix; see timerfd_expire_notify).
    file_id: u64,
}

impl TimerFd {
    fn new(clockid: i32) -> Self {
        Self {
            clockid,
            kernel_timer_id: 0,
            interval_jiffies: 0,
            expiration_count: core::sync::atomic::AtomicU64::new(0),
            wait_queue: crate::process::wait::WaitQueueHead::new(),
            file_id: 0,
        }
    }
}

fn timerfd_read(file: &crate::fs::File, buf: &mut [u8]) -> isize {
    if buf.len() < 8 {
        return -errno::EINVAL as isize;
    }
    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return -errno::EBADF as isize,
    };
    // SAFETY: ptr came from Box::into_raw in sys_timerfd_create; valid and properly aligned.
    let tfd = unsafe { &*(ptr as *const TimerFd) };

    // Blocking read (review批次1: previously EAGAIN — busy loop for every
    // blocking timerfd consumer). Block until an expiration is pending.
    if tfd.expiration_count.load(core::sync::atomic::Ordering::Acquire) == 0 {
        if file.flags().bits() & crate::fs::file::FileFlags::O_NONBLOCK != 0 {
            return -errno::EAGAIN as isize;
        }
        loop {
            let current = match crate::sched::current() {
                Some(t) => t,
                None => return -errno::EAGAIN as isize,
            };
            tfd.wait_queue.prepare_to_wait(current, false, true);
            if tfd.expiration_count.load(core::sync::atomic::Ordering::Acquire) > 0 {
                tfd.wait_queue.finish_wait(current);
                // SAFETY: current is the running task's pointer.
                unsafe { crate::sched::dequeue_task(&*current); }
                break;
            }
            if crate::signal::signal_pending() {
                tfd.wait_queue.finish_wait(current);
                // SAFETY: current is the running task's pointer.
                unsafe { crate::sched::dequeue_task(&*current); }
                return -(errno::EINTR) as isize;
            }
            crate::arch::cpu::restore_irq(true);
            crate::sched::schedule();
            tfd.wait_queue.finish_wait(current);
            if crate::signal::signal_pending() {
                return -(errno::EINTR) as isize;
            }
        }
    }

    // Read and reset the expiration count
    let count = tfd.expiration_count.swap(0, core::sync::atomic::Ordering::AcqRel);
    if count == 0 {
        // Raced with another reader: treat as EAGAIN (nothing pending now).
        return -errno::EAGAIN as isize;
    }

    buf[..8].copy_from_slice(&count.to_le_bytes());
    8
}

fn timerfd_close(file: &crate::fs::File) -> i32 {
    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return 0,
    };
    // SAFETY: ptr came from Box::into_raw in sys_timerfd_create; valid and properly aligned.
    let tfd = unsafe { &*(ptr as *const TimerFd) };

    // Disarm kernel timer
    if tfd.kernel_timer_id != 0 {
        crate::timer::del_timer(tfd.kernel_timer_id);
    }

    // R20-5: free the boxed TimerFd — File has no Drop impl, so the old
    // comment ("freed when File is dropped") was wrong: it leaked. Runs
    // only on the last Arc reference (see eventfd_close).
    // Residual race (pre-existing, documented): a timer that expired in
    // the SAME softirq pass that races this close can deliver one final
    // fetch_add after the free (timer.rs R12-3 snapshot is delivered
    // outside the TIMERS lock). The window is one softirq pass (~µs);
    // before this fix the box leaked on EVERY close, which is strictly
    // worse.
    // SAFETY: private_data is an UnsafeCell; we hold &File for the last
    // reference, so no timerfd_read/settime/gettime can be using it.
    if let Some(ptr) = unsafe { *file.private_data.get() } {
        // SAFETY: ptr came from Box::into_raw in sys_timerfd_create.
        unsafe {
            let _ = alloc::boxed::Box::from_raw(ptr as *mut TimerFd);
        }
        unsafe { *file.private_data.get() = None; }
    }
    0
}

fn timerfd_poll(file: &crate::fs::File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;
    let mut ready = 0u16;

    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return POLLERR,
    };
    // SAFETY: ptr came from Box::into_raw in sys_timerfd_create; valid and properly aligned.
    let tfd = unsafe { &*(ptr as *const TimerFd) };

    let count = tfd.expiration_count.load(core::sync::atomic::Ordering::Acquire);

    if events & POLLIN != 0 && count > 0 {
        ready |= POLLIN | POLLRDNORM;
    }

    ready
}

/// Write old timer settings (for timerfd_gettime / timerfd_settime old_value).
/// Returns false when a user store faulted (callers must surface EFAULT —
/// LTP timerfd_settime01 case 2 passes a bad old_value pointer).
/// Snapshot a timerfd's current settings as the four itimerspec fields
/// (it_interval.sec, it_interval.nsec, it_value.sec, it_value.nsec).
///
/// The reference kernel takes this snapshot after cancelling the old
/// timer and before programming the new one (kernel/fs/timerfd.c:
/// do_timerfd_settime() fills `old` between the cancel loop and
/// timerfd_setup()), so the values describe the state the timer had
/// BEFORE the new settings were applied.
fn timerfd_old_values(tfd: &TimerFd) -> (i64, i64, i64, i64) {
    // it_interval (ns-precise from the stored interval)
    let interval_ms = crate::drivers::timer::jiffies_to_msecs(tfd.interval_jiffies);
    let interval_ns = (interval_ms as u64).saturating_mul(1_000_000);
    // it_value: real remaining time from the wheel
    let (val_sec, val_nsec) = tfd_remaining(tfd);
    (
        (interval_ns / 1_000_000_000) as i64,
        (interval_ns % 1_000_000_000) as i64,
        val_sec,
        val_nsec,
    )
}

/// Copy an itimerspec snapshot out to the user pointer.
///
/// SAFETY: old_value must be validated with access_ok(32 bytes) by the
/// caller; put_user is the exception-table copy path (SUM=0 safe).
fn timerfd_put_olds(old_value: *mut u64, vals: (i64, i64, i64, i64)) -> bool {
    unsafe {
        let p = old_value as *mut i64;
        let put = crate::arch::uaccess::put_user;
        let mut ok = true;
        ok &= put(p, vals.0);
        ok &= put(p.add(1), vals.1);
        ok &= put(p.add(2), vals.2);
        ok &= put(p.add(3), vals.3);
        ok
    }
}

fn timerfd_write_olds(tfd: &TimerFd, old_value: *mut u64) -> bool {
    timerfd_put_olds(old_value, timerfd_old_values(tfd))
}

/// Current (it_value) of a timerfd as (sec, nsec): 0/0 when disarmed.
fn tfd_remaining(tfd: &TimerFd) -> (i64, i64) {
    if tfd.kernel_timer_id == 0 {
        return (0, 0);
    }
    match crate::timer::get_timer_state(tfd.kernel_timer_id) {
        Some((expires, _interval)) => {
            let now = crate::drivers::timer::get_jiffies();
            if expires <= now {
                return (0, 0);
            }
            let ms = crate::drivers::timer::jiffies_to_msecs(expires - now);
            ((ms / 1000) as i64, ((ms % 1000) * 1_000_000) as i64)
        }
        None => (0, 0),
    }
}

/// TimerFd file operations (D-6 builder form)
static TIMERFD_OPS: crate::fs::FileOps = crate::fs::FileOps::builder()
    .read(timerfd_read)
    .close(timerfd_close)
    .poll(timerfd_poll)
    .build();

// ============================================================================
// Poll-wait registration: timerfd / signalfd (P3: event-driven poll)
// ============================================================================

/// Register `task` on the timerfd's wait queue; timerfd_expire_notify
/// wakes it when the timer fires.
fn timerfd_poll_wait(file: &crate::fs::File, task: *mut crate::process::task::Task) -> bool {
    // SAFETY: the dispatcher identity-checked the File against TIMERFD_OPS;
    // private_data was installed by sys_timerfd_create as Box::into_raw and
    // stays valid while the File exists.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return false,
    };
    // SAFETY: ptr came from Box::into_raw(TimerFd).
    let tfd = unsafe { &*(ptr as *const TimerFd) };
    tfd.wait_queue
        .add(crate::process::wait::WaitQueueEntry::new(task, false));
    true
}

/// Remove a registration added by timerfd_poll_wait.
fn timerfd_poll_unwait(file: &crate::fs::File, task: *mut crate::process::task::Task) {
    // SAFETY: same contract as timerfd_poll_wait.
    if let Some(ptr) = unsafe { *file.private_data.get() } {
        let tfd = unsafe { &*(ptr as *const TimerFd) };
        tfd.wait_queue.remove(task);
    }
}

/// Register `task` on the eventfd's wait queue; eventfd_write (counter
/// nonzero) and eventfd_read (space freed) both wake_up_all on this same
/// queue — reference parity for wake_up_poll on the eventfd wqh.
/// Comprehensive-fix audit 2026-11-04: eventfd was the one fd class OH
/// polls (FFRT worker signaling rides eventfd+epoll) that was missing
/// from poll_wait_impls — every poll()/epoll_wait() over an eventfd fell
/// back to the 10 ms re-check slice (latency plus a full fd-set rescan
/// per slice per waiter, the same tax the 26th removed for binder).
fn eventfd_poll_wait(file: &crate::fs::File, task: *mut crate::process::task::Task) -> bool {
    // SAFETY: the dispatcher identity-checked the File against EVENTFD_OPS;
    // private_data was installed by sys_eventfd2 as Box::into_raw and stays
    // valid while the File exists.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return false,
    };
    // SAFETY: ptr came from Box::into_raw(EventFd).
    let efd = unsafe { &*(ptr as *const EventFd) };
    efd.wait_queue
        .add(crate::process::wait::WaitQueueEntry::new(task, false));
    true
}

/// Remove a registration added by eventfd_poll_wait.
fn eventfd_poll_unwait(file: &crate::fs::File, task: *mut crate::process::task::Task) {
    // SAFETY: same contract as eventfd_poll_wait.
    if let Some(ptr) = unsafe { *file.private_data.get() } {
        let efd = unsafe { &*(ptr as *const EventFd) };
        efd.wait_queue.remove(task);
    }
}

/// Register `task` on the signalfd's wait queue; signalfd_notify_task
/// wakes it when a masked signal arrives.
fn signalfd_poll_wait(file: &crate::fs::File, task: *mut crate::process::task::Task) -> bool {
    // SAFETY: the dispatcher identity-checked the File against SIGNALFD_OPS;
    // private_data was installed by sys_signalfd4_impl as Box::into_raw and
    // stays valid while the File exists.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return false,
    };
    // SAFETY: ptr came from Box::into_raw(SignalFd).
    let sfd = unsafe { &*(ptr as *const SignalFd) };
    sfd.wait_queue
        .add(crate::process::wait::WaitQueueEntry::new(task, false));
    true
}

/// Remove a registration added by signalfd_poll_wait.
fn signalfd_poll_unwait(file: &crate::fs::File, task: *mut crate::process::task::Task) {
    // SAFETY: same contract as signalfd_poll_wait.
    if let Some(ptr) = unsafe { *file.private_data.get() } {
        let sfd = unsafe { &*(ptr as *const SignalFd) };
        sfd.wait_queue.remove(task);
    }
}

// ==================== signalfd (P1) ====================

/// signalfd flags (UAPI).
const SFD_CLOEXEC: i32 = 0x80000;
const SFD_NONBLOCK: i32 = 0x800;

/// signalfd_siginfo size (Linux uapi).
const SIGNALFD_SIGINFO_SIZE: usize = 128;

/// signalfd backend: the readable signal set + reader wait queue.
///
/// Semantics boundary (documented simplification): reads consume signals
/// from the READING task's pending set (like rt_sigtimedwait) — the normal
/// usage pattern blocks the mask with sigprocmask first (musl/systemd do),
/// so in-mask signals stay pending in the kernel and are ONLY observable
/// via this fd. If the app does not block them, the normal delivery path
/// may run a handler first and the fd sees nothing ("observer copy"
/// polarity). Signals are NOT hijacked from the handler path.
struct SignalFd {
    /// Signals readable through this fd (bit i-1 = signal i).
    mask: crate::sync::spinlock::Spinlock<u64>,
    /// Readers blocked waiting for a masked signal to arrive.
    wait_queue: crate::process::wait::WaitQueueHead,
}

/// Owning-task → signalfd wait queue registry for signal-arrival wakeups.
/// Task pointers are compared for identity only (never dereferenced), so a
/// recycled task address causes at worst a spurious wake (readers re-check
/// readiness); entries are removed in the fd's close op.
///
/// Wrapper so the raw-pointer registry is `Send` (access under the lock).
struct SignalfdWakeRegistry {
    bindings: alloc::vec::Vec<
        (*mut crate::process::task::Task, *const crate::process::wait::WaitQueueHead),
    >,
}
// SAFETY: task pointers are only compared for identity; queue pointers are
// unregistered in the owning fd's close op before the box is freed.
unsafe impl Send for SignalfdWakeRegistry {}

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
    // Serial-cleanliness: opt-in via dfx=sfddbg.
    if unsafe { (*task).pid() == 1 }
        && crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::SfdDbg)
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
            // SAFETY: wq points into a boxed SignalFd alive until its close
            // op unregisters it (close runs only on the last Arc ref).
            unsafe { (**wq).wake_up_all(); }
        }
    }
}

fn signalfd_register(
    task: *mut crate::process::task::Task,
    wq: *const crate::process::wait::WaitQueueHead,
) {
    let mut registry = SIGNALFD_WAKE_REGISTRY.lock_irqsave();
    if !registry.bindings.iter().any(|(t, q)| *t == task && *q == wq) {
        registry.bindings.push((task, wq));
    }
}

fn signalfd_unregister(wq: *const crate::process::wait::WaitQueueHead) {
    let mut registry = SIGNALFD_WAKE_REGISTRY.lock_irqsave();
    registry.bindings.retain(|(_, q)| *q != wq);
}

/// signalfd read: consume the lowest-numbered pending signal in the fd's
/// mask and return it as a 128-byte `struct signalfd_siginfo`.
///
/// Blocking semantics: with no pending in-mask signal, block on the fd's
/// wait queue (woken by signalfd_notify_task / signals) or return EAGAIN
/// for O_NONBLOCK fds. The consumed signal is removed from pending, so it
/// never reaches a handler (sigtimedwait-style consumption).
fn signalfd_read(file: &crate::fs::File, buf: &mut [u8]) -> isize {
    if buf.len() < SIGNALFD_SIGINFO_SIZE {
        return -errno::EINVAL as isize;
    }
    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent
    // mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return -errno::EBADF as isize,
    };
    // SAFETY: ptr came from Box::into_raw in sys_signalfd4_impl; valid and
    // uniquely owned by this File.
    let sfd = unsafe { &*(ptr as *const SignalFd) };

    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -errno::EAGAIN as isize,
    };

    loop {
        let mask = *sfd.mask.lock();
        // SAFETY: current is the running task's pointer.
        let pending = unsafe { (*current).pending.get_all() };
        let ready = pending & mask;
        if ready != 0 {
            let sig = ready.trailing_zeros() as i32 + 1;
            // SFDDBG (launcher bring-up): a pid-1 signalfd consume — the
            // moment init learns a child died (HandleSigChild follows).
            if (*current).pid() == 1 {
                static RD_N: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                if RD_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 100 {
                    crate::pr_info!("SFDDBG: pid1 read sfd consumed sig={}", sig);
                }
            }
            // Consume (sigtimedwait semantics): clear the bitmap bit and
            // take the queued info when present.
            // SAFETY: current is the running task's pointer.
            let info = unsafe { (*current).pending.remove_one(sig) }
                .unwrap_or_else(|| crate::signal::SigInfo::new(
                    sig, crate::signal::si_code::SI_USER, 0, 0));

            // struct signalfd_siginfo (128B), host-endian like Linux:
            //   ssi_signo @0 u32, ssi_errno @4 i32, ssi_code @8 i32,
            //   ssi_pid   @12 u32, ssi_uid  @16 u32, rest zeroed.
            buf[..SIGNALFD_SIGINFO_SIZE].fill(0);
            buf[0..4].copy_from_slice(&(info.si_signo as u32).to_le_bytes());
            buf[4..8].copy_from_slice(&info.si_errno.to_le_bytes());
            buf[8..12].copy_from_slice(&info.si_code.to_le_bytes());
            buf[12..16].copy_from_slice(&(info.si_pid as u32).to_le_bytes());
            buf[16..20].copy_from_slice(&info.si_uid.to_le_bytes());
            return SIGNALFD_SIGINFO_SIZE as isize;
        }

        // Nothing readable.
        if file.flags().bits() & crate::fs::file::FileFlags::O_NONBLOCK != 0 {
            return -errno::EAGAIN as isize;
        }

        // Block until a signal arrives (prepare → re-check → schedule).
        sfd.wait_queue.prepare_to_wait(current, false, true);
        let mask2 = *sfd.mask.lock();
        // SAFETY: current is the running task's pointer.
        if unsafe { (*current).pending.get_all() } & mask2 != 0 {
            sfd.wait_queue.finish_wait(current);
            // SAFETY: current is the running task's pointer; undo a
            // concurrent wake enqueue (NEW-C2 discipline).
            unsafe { crate::sched::dequeue_task(&*current); }
            continue;
        }
        if crate::signal::signal_pending() {
            sfd.wait_queue.finish_wait(current);
            // SAFETY: see above.
            unsafe { crate::sched::dequeue_task(&*current); }
            return -(errno::EINTR) as isize;
        }
        crate::arch::cpu::restore_irq(true);
        crate::sched::schedule();
        sfd.wait_queue.finish_wait(current);
        if crate::signal::signal_pending() {
            return -(errno::EINTR) as isize;
        }
    }
}

/// signalfd poll: POLLIN iff a pending signal intersects the fd mask.
fn signalfd_poll(file: &crate::fs::File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;

    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent
    // mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return POLLERR,
    };
    // SAFETY: ptr came from Box::into_raw in sys_signalfd4_impl.
    let sfd = unsafe { &*(ptr as *const SignalFd) };

    let current = match crate::sched::current() {
        Some(t) => t,
        None => return 0,
    };
    // SAFETY: current is the running task's pointer.
    let pending = unsafe { (*current).pending.get_all() };
    let mut ready = 0u16;
    if events & POLLIN != 0 && pending & *sfd.mask.lock() != 0 {
        ready |= POLLIN | POLLRDNORM;
    }
    ready
}

fn signalfd_close(file: &crate::fs::File) -> i32 {
    // Runs only on the last Arc reference: no blocked reader can hold the
    // box. Purge the wake-registry entry BEFORE freeing (identity-keyed on
    // the queue address).
    // SAFETY: private_data is an UnsafeCell; we hold &File for the last
    // reference.
    if let Some(ptr) = unsafe { *file.private_data.get() } {
        // SAFETY: ptr came from Box::into_raw in sys_signalfd4_impl.
        unsafe {
            let sfd = &*(ptr as *const SignalFd);
            signalfd_unregister(&sfd.wait_queue as *const _);
            let _ = alloc::boxed::Box::from_raw(ptr as *mut SignalFd);
        }
        unsafe { *file.private_data.get() = None; }
    }
    0
}

/// SignalFd file operations (D-6 builder form)
static SIGNALFD_OPS: crate::fs::FileOps = crate::fs::FileOps::builder()
    .read(signalfd_read)
    .close(signalfd_close)
    .poll(signalfd_poll)
    .build();

/// signalfd4 core (NR 74): create a new signalfd, or re-arm the mask of an
/// existing one (fd >= 0).
///
/// # Arguments
/// - args[0]: fd — -1 to create; otherwise an existing signalfd to update
/// - args[1]: mask — pointer to the sigset_t (u64) readable through the fd
/// - args[2]: sizemask — must equal sizeof(sigset_t) == 8
/// - args[3]: flags — SFD_CLOEXEC | SFD_NONBLOCK
pub fn sys_signalfd4_impl(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let mask_ptr = args[1] as *const u64;
    let sizemask = args[2] as usize;
    let flags = args[3] as i32;

    if sizemask != core::mem::size_of::<u64>() {
        return -(errno::EINVAL as i64);
    }
    if flags & !(SFD_CLOEXEC | SFD_NONBLOCK) != 0 {
        return -(errno::EINVAL as i64);
    }
    if mask_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(mask_ptr as usize, 8) {
        return -(errno::EFAULT as i64);
    }
    // SAFETY: mask_ptr validated with access_ok; get_user is the
    // exception-table copy path (SUM=0 safe).
    let mut mask = match unsafe { crate::arch::uaccess::get_user(mask_ptr) } {
        Some(v) => v,
        None => return -(errno::EFAULT as i64),
    };
    // SIGKILL(9)/SIGSTOP(19) can never be observed via signalfd — drop them
    // from the set like Linux does.
    mask &= !((1u64 << (9 - 1)) | (1u64 << (19 - 1)));

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(errno::EBADF as i64),
    };

    if fd >= 0 {
        // Update path: fd must resolve to an existing signalfd.
        let file = match fdtable.get_file(fd as usize) {
            Some(f) => f,
            None => return -(errno::EBADF as i64),
        };
        // Type-confusion guard before treating private_data as SignalFd.
        if !file.get_ops().is_some_and(|o| core::ptr::eq(o, &SIGNALFD_OPS as *const _)) {
            return -(errno::EINVAL as i64);
        }
        // SAFETY: private_data is an UnsafeCell; we hold &File so no
        // concurrent mutable access.
        let ptr = match unsafe { *file.private_data.get() } {
            Some(p) => p,
            None => return -(errno::EBADF as i64),
        };
        // SAFETY: ptr came from Box::into_raw in the create path.
        let sfd = unsafe { &*(ptr as *const SignalFd) };
        *sfd.mask.lock() = mask;
        // Readiness may have changed (mask widened): wake blocked readers
        // so they re-check.
        sfd.wait_queue.wake_up_all();
        return fd as i64;
    }

    if fd != -1 {
        return -(errno::EBADF as i64);
    }

    // Create path.
    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -(errno::EPERM as i64),
    };
    let sfd = alloc::boxed::Box::new(SignalFd {
        mask: crate::sync::spinlock::Spinlock::new(mask),
        wait_queue: crate::process::wait::WaitQueueHead::new(),
    });
    let sfd_ptr = alloc::boxed::Box::into_raw(sfd) as *mut u8;

    let mut file_flags = crate::fs::file::FileFlags::O_RDWR;
    if flags & SFD_NONBLOCK != 0 {
        file_flags |= crate::fs::file::FileFlags::O_NONBLOCK;
    }
    let file = alloc::sync::Arc::new(crate::fs::File::new(
        crate::fs::file::FileFlags::new(file_flags),
    ));
    file.set_ops(&SIGNALFD_OPS);
    file.set_private_data(sfd_ptr);

    // SAFETY: sfd_ptr came from Box::into_raw above; queue address stable
    // for the box's lifetime.
    unsafe {
        signalfd_register(current, &(*(sfd_ptr as *const SignalFd)).wait_queue as *const _);
    }

    let new_fd = match fdtable.alloc_fd() {
        Some(f) => f,
        None => {
            // SAFETY: sfd_ptr was created via Box::into_raw above; reclaim.
            unsafe {
                signalfd_unregister(&(*(sfd_ptr as *const SignalFd)).wait_queue as *const _);
                let _ = alloc::boxed::Box::from_raw(sfd_ptr as *mut SignalFd);
            }
            return -(errno::EMFILE as i64);
        }
    };
    // SFD_CLOEXEC must be applied AFTER install_fd: install_fd clears any
    // pre-set cloexec bit for the fd number (stale-bit hygiene), so setting
    // it first silently lost the flag — fcntl(F_GETFD) showed no FD_CLOEXEC
    // for a signalfd4(SFD_CLOEXEC) descriptor (LTP signalfd4_01).
    match fdtable.install_fd(new_fd, file) {
        Ok(()) => {
            if flags & SFD_CLOEXEC != 0 {
                crate::fs::set_cloexec_fd(new_fd, true);
            }
            new_fd as i64
        }
        Err(_) => {
            // SAFETY: sfd_ptr was created via Box::into_raw above; reclaim.
            unsafe {
                signalfd_unregister(&(*(sfd_ptr as *const SignalFd)).wait_queue as *const _);
                let _ = alloc::boxed::Box::from_raw(sfd_ptr as *mut SignalFd);
            }
            -(errno::ENOMEM as i64)
        }
    }
}

/// sys_eventfd - Create eventfd object (legacy, no flags)
pub fn sys_eventfd(args: SyscallArgs) -> i64 {
    sys_eventfd2([args[0], 0, 0, 0, 0, 0])
}

/// sys_eventfd2 - Create eventfd object (with flags)
///
/// # Arguments
/// - args[0]: initval - initial value of the counter
/// - args[1]: flags - EFD_CLOEXEC (0x80000), EFD_NONBLOCK (0x800), EFD_SEMAPHORE (0x1)
///
/// # Returns
/// Returns eventfd file descriptor on success, negative error code on failure
pub fn sys_eventfd2(args: SyscallArgs) -> i64 {
    let initval = args[0] as u64;
    let flags = args[1] as u32;

    // Validate flags: only accept EFD_CLOEXEC, EFD_NONBLOCK, EFD_SEMAPHORE
    const EFD_CLOEXEC: u32 = 0x80000;
    const EFD_NONBLOCK: u32 = 0x800;
    const VALID_FLAGS: u32 = EFD_CLOEXEC | EFD_NONBLOCK | EFD_SEMAPHORE;
    if flags & !VALID_FLAGS != 0 {
        return -(errno::EINVAL as i64);
    }

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(errno::EBADF as i64),
    };

    // Create EventFd
    let efd = alloc::boxed::Box::new(EventFd::new(initval, flags & EFD_SEMAPHORE));
    let efd_ptr = alloc::boxed::Box::into_raw(efd) as *mut u8;

    // Build file flags
    let mut file_flags = crate::fs::file::FileFlags::O_RDWR;
    if flags & EFD_NONBLOCK != 0 {
        file_flags |= crate::fs::file::FileFlags::O_NONBLOCK;
    }

    let file = alloc::sync::Arc::new(crate::fs::File::new(crate::fs::file::FileFlags::new(file_flags)));
    file.set_ops(&EVENTFD_OPS);
    file.set_private_data(efd_ptr);

    let fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            // Reclaim EventFd
            // SAFETY: efd_ptr was created via Box::into_raw above; reclaim to free.
            unsafe { let _ = alloc::boxed::Box::from_raw(efd_ptr as *mut EventFd); }
            return -(errno::EMFILE as i64);
        }
    };

    match fdtable.install_fd(fd, file) {
        Ok(()) => {
            // Handle EFD_CLOEXEC via the per-descriptor flag — AFTER the
            // install: install_fd resets the per-fd state, so a set before
            // it was silently lost and fcntl(F_GETFD) read 0 (LTP
            // eventfd2_01's EFD_CLOEXEC round; same wipe pattern the
            // epoll_create1 fix at dispatch.rs already works around).
            if flags & EFD_CLOEXEC != 0 {
                crate::fs::set_cloexec_fd(fd, true);
            }
            fd as i64
        }
        Err(_) => {
            // SAFETY: efd_ptr was created via Box::into_raw above; reclaim to free.
            unsafe { let _ = alloc::boxed::Box::from_raw(efd_ptr as *mut EventFd); }
            -(errno::EMFILE as i64)
        }
    }
}

/// sys_inotify_init1 - Create inotify instance
///
/// # Arguments
/// - args[0]: flags - IN_CLOEXEC, IN_NONBLOCK
///
/// Real implementation (P0-4): the instance (event queue + watch table)
/// lives in kernel/src/fs/inotify.rs; the fd is backed by INOTIFY_OPS.
pub fn sys_inotify_init1(args: SyscallArgs) -> i64 {
    let flags = args[0] as u32;
    match crate::fs::inotify::inotify_init1(flags) {
        Ok(fd) => fd as i64,
        Err(e) => e as i64,
    }
}

/// sys_inotify_add_watch - Add watch to inotify instance
///
/// # Arguments
/// - args[0]: fd - inotify file descriptor
/// - args[1]: pathname - path to watch
/// - args[2]: mask - event mask
pub fn sys_inotify_add_watch(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;
    let pathname_ptr = args[1] as *const u8;
    let mask = args[2] as u32;

    // Read the path from user space (CWD-relative allowed — Linux resolves
    // it against the caller's working directory).
    let mut buf = [0u8; 4096];
    let path = match read_inotify_path(pathname_ptr, &mut buf) {
        Ok(p) => p,
        Err(e) => return e,
    };

    match crate::fs::inotify::inotify_add_watch(fd, &path, mask) {
        Ok(wd) => wd as i64,
        Err(e) => e as i64,
    }
}

/// sys_inotify_rm_watch - Remove watch from inotify instance
///
/// # Arguments
/// - args[0]: fd - inotify file descriptor
/// - args[1]: wd - watch descriptor
pub fn sys_inotify_rm_watch(args: SyscallArgs) -> i64 {
    let fd = args[0] as usize;
    let wd = args[1] as i32;
    match crate::fs::inotify::inotify_rm_watch(fd, wd) {
        Ok(()) => 0,
        Err(e) => e as i64,
    }
}

/// NUL-terminated user-path reader for inotify_add_watch. Returns the
/// (absolute) path or the negative errno as i64.
fn read_inotify_path(ptr: *const u8, buf: &mut [u8; 4096]) -> Result<alloc::string::String, i64> {
    use crate::arch::uaccess::{access_ok, strncpy_from_user};

    if ptr.is_null() {
        return Err(-(errno::EFAULT as i64));
    }
    if !access_ok(ptr as usize, 4096) {
        return Err(-(errno::EFAULT as i64));
    }
    let bytes = match strncpy_from_user(ptr, 4096, buf) {
        Ok(b) => b,
        Err(e) => return Err(e),
    };
    let s = core::str::from_utf8(bytes).map_err(|_| -(errno::EINVAL as i64))?;
    if s.starts_with('/') {
        Ok(alloc::string::String::from(s))
    } else {
        // Relative: resolve against the CWD.
        let cwd = if let Some(current) = crate::sched::current() {
            let cwd_bytes = unsafe { (*current).get_cwd() };
            core::str::from_utf8(&cwd_bytes)
                .map(alloc::string::String::from)
                .unwrap_or_else(|_| alloc::string::String::from("/"))
        } else {
            alloc::string::String::from("/")
        };
        let mut path = cwd;
        if !path.ends_with('/') {
            path.push('/');
        }
        path.push_str(s);
        Ok(path)
    }
}

/// sys_timerfd_create - Create timer file descriptor
///
/// # Arguments
/// - args[0]: clockid - clock ID (CLOCK_REALTIME=0, CLOCK_MONOTONIC=1)
/// - args[1]: flags - TFD_CLOEXEC, TFD_NONBLOCK
pub fn sys_timerfd_create(args: SyscallArgs) -> i64 {
    // lb-lane hunt (syscost census): nr=281 (timerfd_create) ran 111k
    // times in the first 60 s of an OH boot, accelerating to ~4.3k/s by
    // t=240 s — several CPUs of kernel work riding the fd-table and
    // allocator. Name the caller (comm+pid+args+result), budget 60.
    static TFDC_N: core::sync::atomic::AtomicUsize =
        core::sync::atomic::AtomicUsize::new(0);
    let tfdc_i = TFDC_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    // dfx=tfdcensus: WINDOWED caller census. The syscost boots show the
    // storm accelerating 1.9k -> 4.3k/s by t=240s, so a boot-start budget
    // only ever names bluetooth/audio's one-shots. Behind the switch the
    // tap prints the first 16 calls of every 500-jiffy window, so the
    // phase that runs the storm names its caller.
    let tfdc_log = if crate::dfx::switches::enabled(
        crate::dfx::switches::DfxSwitch::TfdCensus,
    ) {
        static WIN_JIF: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        static WIN_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        let jif = crate::drivers::timer::get_jiffies();
        let win = WIN_JIF.load(core::sync::atomic::Ordering::Relaxed);
        if jif.wrapping_sub(win) >= 500
            && WIN_JIF
                .compare_exchange(
                    win,
                    jif,
                    core::sync::atomic::Ordering::AcqRel,
                    core::sync::atomic::Ordering::Relaxed,
                )
                .is_ok()
        {
            WIN_N.store(0, core::sync::atomic::Ordering::Relaxed);
        }
        WIN_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 16
    } else {
        // 110th: the unconditional boot-start budget is gone — the
        // census arms only via dfx=tfdcensus (113 lines rode the
        // pre-launcher window of every perf boot).
        false
    };
    if tfdc_log {
        let comm = crate::sched::current()
            .map(|t| unsafe { (*t).comm() })
            .map(|c| core::str::from_utf8(c).unwrap_or(""))
            .unwrap_or("")
            .trim_end_matches('\0');
        crate::pr_debug!(
                "TFDC[{}] clockid={} flags={:#x} comm='{}' pid={}",
            tfdc_i,
            args[0] as i32,
            args[1] as i32,
            comm,
            crate::process::current_pid()
        );
    }
    let rc = sys_timerfd_create_inner(args);
    if tfdc_log {
        crate::pr_debug!(
                "TFDC[{}] -> {}", tfdc_i, rc);
    }
    rc
}

fn sys_timerfd_create_inner(args: SyscallArgs) -> i64 {
    let clockid = args[0] as i32;
    let flags = args[1] as i32;

    // Only CLOCK_REALTIME and CLOCK_MONOTONIC supported
    if clockid != 0 && clockid != 1 {
        return -(errno::EINVAL as i64);
    }

    // Validate flags
    const TFD_CLOEXEC: i32 = 0x80000;
    const TFD_NONBLOCK: i32 = 0x800;
    if flags & !(TFD_CLOEXEC | TFD_NONBLOCK) != 0 {
        return -(errno::EINVAL as i64);
    }

    let fdtable = match crate::sched::get_current_fdtable() {
        Some(ft) => ft,
        None => return -(errno::EMFILE as i64),
    };

    // Create TimerFd
    let tfd = alloc::boxed::Box::new(TimerFd::new(clockid));
    let tfd_ptr = alloc::boxed::Box::into_raw(tfd) as *mut u8;

    // Build file flags
    let mut file_flags = crate::fs::file::FileFlags::O_RDONLY;
    if flags & TFD_NONBLOCK != 0 {
        file_flags |= crate::fs::file::FileFlags::O_NONBLOCK;
    }

    let file = alloc::sync::Arc::new(crate::fs::File::new(crate::fs::file::FileFlags::new(file_flags)));
    file.set_ops(&TIMERFD_OPS);
    file.set_private_data(tfd_ptr);
    // Bind the TimerFd to its File's generation id so the expiry path
    // can notify the epoll registry (the EPOLLET lost-edge fix: without
    // this notify, an expiry that lands while NO task sleeps in
    // epoll_wait never re-armed the ET edge — last_reported stayed set,
    // and every later epoll_wait suppressed the readable fd. Reproduced
    // by test/tfd_et_probe: FFRT's DelayedWorker pattern, 59/60
    // after-gap expiries lost; Linux reports 300/300).
    // SAFETY: tfd_ptr is the Box::into_raw TimerFd created above; the
    // File is not yet shared (no fd installed), so no concurrent access.
    unsafe {
        (*(tfd_ptr as *mut TimerFd)).file_id = file.file_id;
    }

    let fd = match fdtable.alloc_fd() {
        Some(fd) => fd,
        None => {
            // SAFETY: tfd_ptr was created via Box::into_raw above; reclaim to free.
            unsafe { let _ = alloc::boxed::Box::from_raw(tfd_ptr as *mut TimerFd); }
            return -(errno::EMFILE as i64);
        }
    };

    match fdtable.install_fd(fd, file) {
        Ok(()) => {
            // TFD_CLOEXEC must be set AFTER install_fd — install CLEARS the
            // per-descriptor cloexec bit (LTP timerfd02: the flag set
            // before the install was wiped and F_GETFD read 0).
            if flags & TFD_CLOEXEC != 0 {
                crate::fs::set_cloexec_fd(fd, true);
            }
            fd as i64
        }
        Err(_) => {
            // SAFETY: tfd_ptr was created via Box::into_raw above; reclaim to free.
            unsafe { let _ = alloc::boxed::Box::from_raw(tfd_ptr as *mut TimerFd); }
            -(errno::EMFILE as i64)
        }
    }
}

/// sys_timerfd_settime - Set timer settings
///
/// # Arguments
/// - args[0]: fd - timerfd file descriptor
/// - args[1]: flags - TFD_TIMER_ABSTIME
/// - args[2]: new_value - new timer settings (struct itimerspec, 32 bytes)
/// - args[3]: old_value - old timer settings (output)
pub fn sys_timerfd_settime(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let flags = args[1] as i32;
    let new_value = args[2] as *const u64;
    let old_value = args[3] as *mut u64;

    if new_value.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(new_value as usize, 32) {
        return -(errno::EFAULT as i64);
    }

    // R32-B10: validate flags — only TFD_TIMER_ABSTIME and
    // TFD_TIMER_CANCEL_ON_SET are legal (reference TFD_SETTIME_FLAGS);
    // garbage bits were silently treated as relative mode. CANCEL_ON_SET
    // only has semantics for realtime-derived clocks (cancellation on
    // clock_settime); for CLOCK_MONOTONIC the reference accepts it as a
    // no-op — rejecting it broke LTP timerfd_settime02 (the
    // CVE-2017-10661 regression test arms ABSTIME|CANCEL_ON_SET and
    // expects success).
    // CANCEL_ON_SET is bit 1 (value 2) per the uapi header and
    // musl/glibc alike — a stale note had it as bit 2, so LTP
    // timerfd_settime02 (which arms ABSTIME|CANCEL_ON_SET through musl's
    // constant, value 3) died on EINVAL. Verified against the host
    // reference kernel: flags=3 accepted, flags=5 rejected. The flag only
    // has semantics for realtime-derived clocks (cancellation on
    // clock_settime); accepting it for CLOCK_MONOTONIC as a no-op matches
    // the reference.
    const TFD_TIMER_ABSTIME: i32 = 1;
    const TFD_TIMER_CANCEL_ON_SET: i32 = 1 << 1;
    if flags & !(TFD_TIMER_ABSTIME | TFD_TIMER_CANCEL_ON_SET) != 0 {
        return -(errno::EINVAL as i64);
    }

    // Validate fd and get file
    // SAFETY: fd is a valid timerfd file descriptor from timerfd_create.
    let file = match unsafe { crate::fs::get_file_fd(fd as usize) } {
        Some(f) => f,
        None => return -(errno::EBADF as i64),
    };

    // Verify the fd really is a timerfd before treating private_data as
    // *mut TimerFd (type-confusion guard).
    if !file.get_ops().is_some_and(|o| core::ptr::eq(o, &TIMERFD_OPS as *const _)) {
        return -(errno::EINVAL as i64);
    }

    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return -(errno::EBADF as i64),
    };
    // SAFETY: ptr came from Box::into_raw in sys_timerfd_create; valid and unique.
    let tfd = unsafe { &mut *(ptr as *mut TimerFd) };

    // Read struct itimerspec { struct timespec it_interval, struct timespec it_value }
    //
    // The reference reads the new value FIRST (kernel/fs/timerfd.c:
    // get_itimerspec64() → EFAULT) and only then touches the timer state.
    // SAFETY: new_value validated with access_ok(32 bytes); get_user is the
    // exception-table copy path (SUM=0 safe). Unreadable fields read as 0.
    let (int_sec, int_nsec, val_sec, val_nsec) = unsafe {
        let p = new_value as *const i64;
        let get = crate::arch::uaccess::get_user::<i64>;
        (
            get(p).unwrap_or(0),
            get(p.add(1)).unwrap_or(0),
            get(p.add(2)).unwrap_or(0),
            get(p.add(3)).unwrap_or(0),
        )
    };

    // Linux (timerfd_rearm path): tv_nsec of BOTH members must be a
    // normalized [0, 1e9) value and tv_sec non-negative — anything else is
    // EINVAL (LTP timerfd_settime02).
    //
    // The check comes BEFORE the old timer is cancelled: the reference
    // validates itimerspec64_valid() before it cancels the hrtimer, so a
    // rejected call leaves the running timer alone. Cancelling first
    // silently disarmed the caller's timer on every rejected call.
    if int_sec < 0 || int_nsec < 0 || int_nsec > 999_999_999
        || val_sec < 0 || val_nsec < 0 || val_nsec > 999_999_999
    {
        return -(errno::EINVAL as i64);
    }

    // Snapshot the settings the timer had BEFORE the reprogramming (the
    // reference fills `old` between the cancel and the setup).
    let old_values = timerfd_old_values(tfd);

    // Disarm existing timer
    if tfd.kernel_timer_id != 0 {
        crate::timer::del_timer(tfd.kernel_timer_id);
        tfd.kernel_timer_id = 0;
    }

    // Reference timerfd_setup parity (kernel/linux/linux-6.6/fs/timerfd.c
    // timerfd_setup: `ctx->expired = 0; ctx->ticks = 0;` before the arm —
    // called from do_timerfd_settime on EVERY re-programming): a
    // successful settime DISCARDS the pending unread expirations. Without
    // the reset the timerfd stays readable FOREVER after its first expiry
    // and the OH timer loops (epoll_pwait returning instantly + settime
    // rearm, the TimeOut read never scheduled) churn at CPU rate instead
    // of sleeping until the new deadline — the u2v5 ring named the loop
    // (pid 588/693/694: epoll_pwait ret=1 + timerfd_settime +
    // clock_gettime, no read of the timerfd), and the same stale
    // readiness feeds the clock_gettime/epoll syscost storms (66k/s,
    // ~10k/s).
    tfd.expiration_count.store(0, core::sync::atomic::Ordering::Release);

    // If value is zero, timer is disarmed. The old settings are still
    // copied out in that case (the reference programs the timer - or does
    // not - and copies `old` out afterwards in both paths).
    let total_nsec = val_sec.saturating_mul(1_000_000_000).saturating_add(val_nsec); // M-19

    // Convert to jiffies
    let value_msecs = (total_nsec / 1_000_000) as u64;
    let value_jiffies = crate::drivers::timer::msecs_to_jiffies(value_msecs).max(1);

    let interval_nsec = int_sec.saturating_mul(1_000_000_000).saturating_add(int_nsec); // M-19
    let interval_jiffies = if interval_nsec > 0 {
        let interval_msecs = (interval_nsec / 1_000_000) as u64;
        crate::drivers::timer::msecs_to_jiffies(interval_msecs).max(1)
    } else {
        0
    };

    // R32-B10 fix: for TFD_TIMER_ABSTIME, it_value is an ABSOLUTE time on
    // the timer's clock. It CANNOT be interpreted as an absolute jiffies
    // count — jiffies are based at the FIRST timer tick (seconds after
    // mtime-reset), so `msecs_to_jiffies(abs_ms)` lands far in the past or
    // future depending on boot timing (LTP timerfd01: an absolute now+50ms
    // timer read back as multiple seconds remaining and delivered one
    // tick). Convert to a RELATIVE deadline first (REALTIME values shift
    // back by the wall epoch), then add to the current jiffies. A deadline
    // already in the past yields expires == now and fires on the next
    // softirq scan — matching Linux's immediate expiry.
    let now = crate::drivers::timer::get_jiffies();
    let expires = if flags & 1 != 0 {
        let freq_hz: u64 = crate::config::TIMER_CLOCK_FREQ_HZ;
        let now_ns = (crate::drivers::timer::read_time() as u64)
            .saturating_mul(1_000_000_000 / freq_hz);
        let abs_ns = (val_sec.max(0) as u64)
            .saturating_mul(1_000_000_000)
            .saturating_add(val_nsec.max(0) as u64);
        let abs_mono_ns = if tfd.clockid == 0 {
            // CLOCK_REALTIME: shift into the monotonic domain.
            abs_ns.saturating_sub(crate::syscall::time::wall_epoch_offset_ns())
        } else {
            abs_ns
        };
        let rel_ns = abs_mono_ns.saturating_sub(now_ns);
        if rel_ns == 0 {
            now
        } else {
            let rel_ms = rel_ns / 1_000_000;
            now.saturating_add(crate::drivers::timer::msecs_to_jiffies(rel_ms).max(1))
        }
    } else {
        now + value_jiffies
    };

    // Use timerfd mode: pass the TimerFd address to the timer softirq; on
    // expiry it calls timerfd_expire_notify() which bumps the counter AND
    // wakes readers blocked in timerfd_read (review批次1: the counter bump
    // alone never woke anyone, so a blocking read would sleep forever).
    let tfd_addr = tfd as *const TimerFd as u64;

    if total_nsec > 0 {
        let new_kernel_id = crate::timer::add_timer_with_action(
            expires,
            0, // no signal
            0, // no signal
            interval_jiffies,
            tfd_addr,
        );

        tfd.kernel_timer_id = new_kernel_id;
    }

    // The reference updates the interval and clears the tick counter in
    // BOTH cases - timerfd_setup() sets ctx->tintv and ctx->ticks = 0
    // before it decides whether to start the hrtimer
    // (kernel/fs/timerfd.c: timerfd_setup()). A disarm therefore keeps
    // the NEW interval and clears the expirations.
    tfd.interval_jiffies = interval_jiffies;
    tfd.expiration_count.store(0, core::sync::atomic::Ordering::Relaxed);

    // Copy the old settings out LAST: the reference copies them after the
    // timer has been reprogrammed (kernel/fs/timerfd.c: do_timerfd_settime()
    // then put_itimerspec64()), so a failing copy returns EFAULT with the
    // new timer already programmed. A faulting store is EFAULT (LTP
    // timerfd_settime01 case 2 passes a bad old_value pointer and the old
    // code silently swallowed the failure).
    if !old_value.is_null() {
        if !crate::arch::uaccess::access_ok(old_value as usize, 32) {
            return -(errno::EFAULT as i64);
        }
        if !timerfd_put_olds(old_value, old_values) {
            return -(errno::EFAULT as i64);
        }
    }

    0
}

/// Timer-softirq expiry hook: bump the timerfd's expiration counter and wake
/// readers blocked in timerfd_read. `tfd_addr` is the TimerFd address that
/// sys_timerfd_settime registered in the timer action.
///
/// SAFETY contract (same accepted close-race as the old direct counter bump):
/// the TimerFd box lives as long as the File's last Arc reference; a close
/// racing this delivery is bounded to one softirq pass and audited.
pub fn timerfd_expire_notify(tfd_addr: u64) {
    if tfd_addr == 0 {
        return;
    }
    // SAFETY: see contract above.
    unsafe {
        let tfd = &*(tfd_addr as *const TimerFd);
        tfd.expiration_count.fetch_add(1, core::sync::atomic::Ordering::Release);
        tfd.wait_queue.wake_up_all();
        // EPOLLET lost-edge fix (2026-11-06): the wait_queue wake above
        // only reaches tasks REGISTERED on this fd's queue (an
        // epoll_wait sleeper via timerfd_poll_wait). An expiry that
        // lands while NOBODY waits must still re-arm the edge-triggered
        // interests and wake epoll sleepers — the reference's
        // ep_poll_callback runs from wake_up_poll on the file's wait
        // queue unconditionally. Without this notify, the ET entry's
        // last_reported mask stayed set and every later epoll_wait
        // suppressed the readable fd (the FFRT DelayedWorker pattern:
        // timerfd NONBLOCK + EPOLLET + never read + expiry between
        // waits — 59/60 lost; Linux 300/300). Same notify discipline as
        // the unix-socket readiness sites.
        if tfd.file_id != 0 {
            crate::syscall::misc::epoll_notify_file(tfd.file_id);
        }
    }
}

/// sys_timerfd_gettime - Get timer settings
///
/// # Arguments
/// - args[0]: fd - timerfd file descriptor
/// - args[1]: curr_value - current timer settings (output, 32 bytes)
pub fn sys_timerfd_gettime(args: SyscallArgs) -> i64 {
    let fd = args[0] as i32;
    let curr_value = args[1] as *mut u64;

    // Linux checks the fd FIRST (LTP timerfd_gettime01 case 1 passes
    // fd=-1 with a NULL buffer and expects EBADF, not EFAULT), then the
    // not-a-timerfd EINVAL, and only then touches the user buffer.
    // SAFETY: fd is a valid timerfd file descriptor from timerfd_create.
    let file = match unsafe { crate::fs::get_file_fd(fd as usize) } {
        Some(f) => f,
        None => return -(errno::EBADF as i64),
    };

    // Verify the fd really is a timerfd before treating private_data as
    // *const TimerFd (type-confusion guard).
    if !file.get_ops().is_some_and(|o| core::ptr::eq(o, &TIMERFD_OPS as *const _)) {
        return -(errno::EINVAL as i64);
    }

    if curr_value.is_null() {
        return -(errno::EFAULT as i64);
    }
    if !crate::arch::uaccess::access_ok(curr_value as usize, 32) {
        return -(errno::EFAULT as i64);
    }

    // SAFETY: private_data is an UnsafeCell; we hold &File so no concurrent mutable access.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return -(errno::EBADF as i64),
    };
    // SAFETY: ptr came from Box::into_raw in sys_timerfd_create; valid and properly aligned.
    let tfd = unsafe { &*(ptr as *const TimerFd) };

    if !timerfd_write_olds(tfd, curr_value) {
        return -(errno::EFAULT as i64);
    }
    0
}

/// sys_getrandom - Get random bytes
///
/// # Arguments
/// - args[0]: buf - buffer to store random bytes
/// - args[1]: buflen - number of bytes requested
/// - args[2]: flags - flags (GRND_NONBLOCK, GRND_RANDOM, etc.)
///
/// # Returns
/// Returns number of bytes written on success, negative error code on failure
pub fn sys_getrandom(args: SyscallArgs) -> i64 {
    let buf_ptr = args[0] as *mut u8;
    let buflen = args[1] as usize;
    let flags = args[2] as u32;

    // Flags validation (Linux): GRND_RANDOM 0x1, GRND_NONBLOCK 0x2,
    // GRND_INSECURE 0x4 — anything else is EINVAL (LTP getrandom05).
    const GRND_VALID: u32 = 0x1 | 0x2 | 0x4;
    if flags & !GRND_VALID != 0 {
        return -(errno::EINVAL as i64);
    }

    if buf_ptr.is_null() {
        return -(errno::EFAULT as i64);
    }

    if buflen == 0 {
        return 0;
    }

    // Validate user space pointer
    if !crate::arch::uaccess::access_ok(buf_ptr as usize, buflen) {
        return -(errno::EFAULT as i64);
    }

    // ChaCha20-based CRNG (review批次1: the old LCG was trivially
    // predictable — a fatal weakness for TLS/SSH seeding).
    // Stage each block in a kernel buffer and go through copy_to_user:
    // a raw memcpy to a U-bit page faults in S-mode (SUM=0) and the
    // unannotated fault escalates to a kernel panic (Ubuntu boot hit).
    let mut guard = GETRANDOM_CRNG.lock();
    let rng = guard.get_or_insert_with(ChaCha20Crng::new);
    let mut filled = 0usize;
    while filled < buflen {
        let (block, used) = rng.next_bytes();
        let chunk = (buflen - filled).min(64 - used);
        // SAFETY: buf_ptr validated with access_ok(buflen); copy_to_user
        // is exception-table fault-safe and returns uncopied count.
        let uncopied = unsafe {
            crate::arch::uaccess::copy_to_user(
                buf_ptr.add(filled),
                block.as_ptr().add(used),
                chunk,
            )
        };
        if uncopied != 0 {
            return if filled > 0 {
                filled as i64
            } else {
                -(errno::EFAULT as i64)
            };
        }
        rng.consume(chunk);
        filled += chunk;
    }

    buflen as i64
}

/// ChaCha20 keystream generator backing sys_getrandom.
///
/// Construction: a 256-bit key is drawn once per boot from CLINT timebase,
/// jiffies, and (best-effort) address-entropy; every 64-byte block is then
/// `ChaCha20(key, counter, nonce=fresh_entropy)` so no keystream block ever
/// repeats even across counter reuse, and each call is re-keyed by fresh
/// timer entropy (the evolving public nonce cannot be used to recover the
/// key — ChaCha20 acts as a PRF here).
struct ChaCha20Crng {
    key: [u32; 8],
    counter: u64,
    /// Partially consumed keystream block (index into `block`).
    block: [u8; 64],
    pos: usize,
}

impl ChaCha20Crng {
    /// Quarter round on four state words.
    #[inline]
    fn qr(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
        s[a] = s[a].wrapping_add(s[b]);
        s[d] ^= s[a];
        s[d] = s[d].rotate_left(16);
        s[c] = s[c].wrapping_add(s[d]);
        s[b] ^= s[c];
        s[b] = s[b].rotate_left(12);
        s[a] = s[a].wrapping_add(s[b]);
        s[d] ^= s[a];
        s[d] = s[d].rotate_left(8);
        s[c] = s[c].wrapping_add(s[d]);
        s[b] ^= s[c];
        s[b] = s[b].rotate_left(7);
    }

    /// One 20-round ChaCha20 block from key/counter/nonce.
    fn block(key: &[u32; 8], counter: u64, nonce: u64) -> [u8; 64] {
        const CONSTANTS: [u32; 4] =
            [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];
        let mut s = [
            CONSTANTS[0],
            CONSTANTS[1],
            CONSTANTS[2],
            CONSTANTS[3],
            key[0],
            key[1],
            key[2],
            key[3],
            key[4],
            key[5],
            key[6],
            key[7],
            counter as u32,
            (counter >> 32) as u32,
            nonce as u32,
            (nonce >> 32) as u32,
        ];
        let mut w = s;
        for _ in 0..10 {
            // Column rounds
            Self::qr(&mut w, 0, 4, 8, 12);
            Self::qr(&mut w, 1, 5, 9, 13);
            Self::qr(&mut w, 2, 6, 10, 14);
            Self::qr(&mut w, 3, 7, 11, 15);
            // Diagonal rounds
            Self::qr(&mut w, 0, 5, 10, 15);
            Self::qr(&mut w, 1, 6, 11, 12);
            Self::qr(&mut w, 2, 7, 8, 13);
            Self::qr(&mut w, 3, 4, 9, 14);
        }
        let mut out = [0u8; 64];
        for i in 0..16 {
            let x = w[i].wrapping_add(s[i]);
            out[i * 4..i * 4 + 4].copy_from_slice(&x.to_le_bytes());
        }
        out
    }

    /// Mix 64 bits of entropy into a u64 (splitmix64 finalizer).
    #[inline]
    fn mix64(x: u64) -> u64 {
        let mut z = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Seed a fresh CRNG from the best entropy available pre-boot-rng:
    /// CLINT timebase, jiffies, a monotonically increasing boot counter,
    /// and the address of a stack local (ASLR/scheduler placement).
    fn new() -> Self {
        let clint = crate::arch::cpu::read_time();
        let jiffies = crate::drivers::timer::get_jiffies() as u64;
        static BOOT_NONCE: core::sync::atomic::AtomicU64 =
            core::sync::atomic::AtomicU64::new(0);
        let stack_addr = &clint as *const _ as u64;
        let boot = BOOT_NONCE.fetch_add(1, core::sync::atomic::Ordering::Relaxed);

        let mut key = [0u32; 8];
        let e = [
            clint,
            clint >> 13, // spread the fast-moving bits
            jiffies,
            jiffies ^ 0xa5a5_5a5a_a5a5_5a5a,
            boot,
            stack_addr,
            stack_addr >> 3,
            clint ^ (jiffies << 17) ^ (boot << 41),
        ];
        for (i, v) in e.iter().enumerate() {
            key[i] = Self::mix64(*v) as u32;
            key[(i + 4) % 8] ^= (Self::mix64(*v ^ 0xdead_beef_dead_beef) >> 32) as u32;
        }
        ChaCha20Crng { key, counter: Self::mix64(clint ^ boot), block: [0; 64], pos: 64 }
    }

    /// Return the current (possibly partially consumed) keystream block and
    /// the consumed offset. Generates a fresh block when the previous one is
    /// exhausted, with a nonce drawn from fresh CLINT entropy.
    fn next_bytes(&mut self) -> ([u8; 64], usize) {
        if self.pos >= 64 {
            let entropy = crate::arch::cpu::read_time();
            self.block = Self::block(&self.key, self.counter, Self::mix64(entropy));
            self.counter = self.counter.wrapping_add(1);
            self.pos = 0;
        }
        (self.block, self.pos)
    }

    /// Mark `n` bytes of the current block as consumed.
    fn consume(&mut self, n: usize) {
        self.pos = (self.pos + n).min(64);
    }
}

/// Global getrandom CRNG state (lazily seeded on first use).
static GETRANDOM_CRNG: crate::sync::spinlock::Spinlock<Option<ChaCha20Crng>> =
    crate::sync::spinlock::Spinlock::new(None);


/// Best-effort eventpoll_release: when a file description is closed, purge
/// that fd number from every epoll instance still held by the calling
/// process, so stale entries cannot report EPOLLERR|EPOLLHUP forever
/// (review批次1: close 后 epoll 条目不删; Linux removes entries on the
/// file's last release).
/// (review-s2b perf) Live epoll instances in the whole system. Pure fast-out
/// gate for epoll_purge_closed_fd: when zero, no interest entries can exist
/// anywhere, so the per-close for_each_file walk (entry-lock irqsave pair +
/// slot clones) is pure overhead — the lmbench open case (open+close of a
/// plain file, no epoll in the image) paid it on EVERY close. Inc'd after a
/// successful install in sys_epoll_create, dec'd in epoll_file_close (the
/// last-reference close op — R20-5 discipline). A stale nonzero value (a
/// leaked epoll File) only keeps the purge walk alive, never wrong.
static EPOLL_INSTANCES: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

pub fn epoll_purge_closed_fd(fd: usize, dying_file_id: u64) {
    if EPOLL_INSTANCES.load(core::sync::atomic::Ordering::Acquire) == 0 {
        return;
    }
    // Linux eventpoll_release: closing a file descriptor removes every
    // epoll interest still keyed to that fd number and open file
    // description. The old stub left the entry in place, and the wait
    // path's stale branch then reported EPOLLERR|EPOLLHUP on EVERY
    // epoll_wait — a permanent phantom wakeup that spun GLib main loops
    // (the dbus leftover-ready-fd burn: poll returns ready, the handler
    // finds nothing actionable, the loop repeats). Purge the entry at
    // close, exactly like Linux removes the interest when the fd's last
    // table reference goes away.
    //
    // 79th (ohw124/125, the 8250 wedge's founder): the purge must key
    // on the DYING OPEN FILE DESCRIPTION (dying_file_id), never on the
    // bare fd number. A fork-inherited epoll is ONE EpollFile shared by
    // parent and child, and their fd NUMBERINGS diverge the moment the
    // child execs (CLOEXEC sweep) and reuses numbers for its own files.
    // chipset_init (ohw125 EPPURGE #3) closed its own fd 21 — the
    // closer's-table liveness check saw the cleared slot and stripped
    // init's live (fd=21, paramservice child) interest from the shared
    // instance; init's LoopEvent never read that connection again and
    // every later samgr SystemSetParameter burned its 15 s SO_RCVTIMEO
    // (the app_fwk_update_service 105 loop under the processLock — the
    // whole install-stream wedge). Linux keys the epi to the
    // description (epi->ffd.file, itself referenced), so a foreign
    // process's same-numbered close can never remove it; matching
    // BOTH fd and dying_file_id restores that invariant: the entry
    // dies only when the description it watches dies.
    let fdtable = match crate::sched::current() {
        Some(t) => unsafe { (*t).try_fdtable() },
        None => return,
    };
    let fdtable = match fdtable {
        Some(f) => f,
        None => return,
    };

    // Epoll entries carry the CLOSER's fd numbers, so only epoll
    // instances this task still holds can watch the closed number.
    // Snapshot their EpollFile pointers under one fdtable lock hold;
    // reading private_data needs no other lock (identity-checked below).
    let mut epoll_ptrs: alloc::vec::Vec<*mut EpollFile> = alloc::vec::Vec::new();
    fdtable.for_each_file(|_fdnum, file| {
        if let Some(ops) = file.get_ops() {
            if core::ptr::eq(ops as *const _, &EPOLL_OPS as *const _) {
                // SAFETY: read-only snapshot of the raw pointer; the
                // instance stays alive while this File is in the fd table
                // (its close op only runs on the last Arc reference).
                if let Some(p) = unsafe { *file.private_data.get() } {
                    epoll_ptrs.push(p as *mut EpollFile);
                }
            }
        }
    });
    if epoll_ptrs.is_empty() {
        return;
    }

    for ep in epoll_ptrs {
        // SAFETY: the instance is alive (its owning File holds an Arc in
        // the fd table we just scanned; the box is freed only in its close
        // op, which cannot run while that Arc exists).
        let epoll = unsafe { &*ep };
        // At most ONE entry can match (fd, dying_file_id): entries are
        // unique per fd number (ADD dedups) and file_id is never reused —
        // so a node pointer + its id replace the old removed_ids Vec.
        let mut dead: core::option::Option<*mut EpollEntry> = None;
        let mut removed_id: u64 = 0;
        {
            let mut entries = epoll.entries.lock();
            // Intrusive-list walk (117th): unlink the matching dead
            // interest under the lock (two pointer writes — the old
            // Vec::retain compacted the whole set); the node free runs
            // OUTSIDE the lock.
            let head = entries.init_once();
            // SAFETY: nodes are linked and unlinked only under this lock.
            let mut cur = unsafe { (*head).next };
            while cur != head {
                let next = unsafe { (*cur).next };
                // SAFETY: cur is a linked EpollEntry's `list` (first
                // field).
                let e = unsafe { &mut *(cur as *mut EpollEntry) };
                cur = next;
                // 79th: remove ONLY the dying description's entries. A
                // same-numbered entry for a DIFFERENT description (the
                // fork-shared instance's foreign owner — see the function
                // doc) must survive this close untouched.
                if e.fd as usize != fd || e.file_id != dying_file_id {
                    continue;
                }
                // The fd slot is empty at this point (close_fd cleared it
                // before we run). If it somehow resolves again to the SAME
                // description, the entry is live (immediate-reuse race);
                // anything else is the closed interest — remove it.
                let live = fdtable
                    .get_file(fd)
                    .map_or(false, |f| f.file_id == e.file_id);
                if !live {
                    // SAFETY: e.list is linked in this epoll's list.
                    unsafe { e.list.del(); }
                    removed_id = e.file_id;
                    dead = Some(e as *mut EpollEntry);
                }
                break; // fd-number-unique: no further match can exist
            }
        }
        if let Some(node) = dead {
            // Free the unlinked node with NO entries lock held (117th
            // allocation discipline — the free can hit the allocator's
            // slow path, which must never run under the lock).
            // SAFETY: unlinked above under the lock; interest nodes are
            // reachable only through the epoll's entries list.
            unsafe { drop(alloc::boxed::Box::from_raw(node)); }
            // Registry lock only after the entries lock is released (the
            // documented nesting rule — registry helpers never nest
            // entries).
            // 79th (ohw124): attribute every purge removal — the
            // paramservice dark-child hunt needs the closer's pid/fd/
            // file_id triad.
            if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::Pid1Spin) {
                static PURGE2_N: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                let n = PURGE2_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < 300 {
                    crate::pr_warn!(
                        "EPPURGE #{} pid={} fd={} fids={:#x}",
                        n,
                        crate::process::current_pid(),
                        fd,
                        removed_id
                    );
                }
            }
            epoll_wake_unregister(removed_id, ep);
        }
    }
}

/// 53rd session (wedge chain: timer-driven event loops): timerfd wheel
/// audit. One line per ARMED timerfd-backed kernel timer: expiry jiffies
/// vs now (overdue ms), interval, wake pid, and the TimerFd wait-queue
/// census (entries/woken). The OH boot's progress engine is FFRT delay
/// queues + event runners driven by timerfd+epoll; a delay thread parked
/// in timerfd_poll_wait while its wheel entry is long OVERDUE (or has
/// vanished without delivering) is the lost-timer class — and the
/// year-timeout condvar pandemic upstream of it (52nd §6a descent).
/// Piggybacks on dfx=sleepsnap cadence (self-gated). Deduped on
/// (tfd_addr, expires): each re-arm prints once; an unchanged overdue
/// state prints once and stays named by its first line.
pub fn timerfd_wheel_audit() {
    if !crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::SleepSnap) {
        return;
    }
    use core::fmt::Write as _;
    let now = crate::drivers::timer::get_jiffies();
    let mut out = alloc::string::String::new();
    for t in crate::timer::snapshot_timers() {
        if t.tfd_addr == 0 {
            continue;
        }
        // Dedup ring on (tfd_addr, expires).
        {
            const N: usize = 128;
            static A: [core::sync::atomic::AtomicU64; N] =
                [const { core::sync::atomic::AtomicU64::new(0) }; N];
            static E: [core::sync::atomic::AtomicU64; N] =
                [const { core::sync::atomic::AtomicU64::new(0) }; N];
            static CUR: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let mut seen = false;
            for i in 0..N {
                if A[i].load(core::sync::atomic::Ordering::Relaxed) == t.tfd_addr
                    && E[i].load(core::sync::atomic::Ordering::Relaxed) == t.expires
                {
                    seen = true;
                    break;
                }
            }
            if seen {
                continue;
            }
            let idx = CUR.fetch_add(1, core::sync::atomic::Ordering::Relaxed) % N;
            A[idx].store(t.tfd_addr, core::sync::atomic::Ordering::Relaxed);
            E[idx].store(t.expires, core::sync::atomic::Ordering::Relaxed);
        }
        // SAFETY: tfd_addr is pinned by the timer's armed action (the
        // disarming paths — settime/cancel — detach it first); read-only
        // diagnostic access to the count atom and the wait queue.
        let tfd = unsafe { &*(t.tfd_addr as *const TimerFd) };
        let cnt = tfd.expiration_count.load(core::sync::atomic::Ordering::Acquire);
        let (wq_total, wq_woken) = tfd.wait_queue.vw_queue_census();
        let overdue_ms = if t.expires > now {
            0
        } else {
            (now - t.expires) * 1000 / crate::config::KERNEL_HZ as u64
        };
        let _ = writeln!(
            out,
            "tfdw: tfd={:#x} expires={} now={} overdue={}ms ivl={} cnt={} wq={}/{} wake={}",
            t.tfd_addr,
            t.expires,
            now,
            overdue_ms,
            t.interval_jiffies,
            cnt,
            wq_woken,
            wq_total,
            t.wake_pid
        );
    }
    if !out.is_empty() {
        let mut rest = out.as_str();
        while !rest.is_empty() {
            let mut cut = rest.len().min(240);
            if cut < rest.len() {
                // ohc47: boundary backoff BEFORE the rfind slice (a
                // multi-byte char straddling cut panics rest[..cut]).
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
            crate::pr_info!("{}", chunk);
            rest = tail;
        }
    }
}
