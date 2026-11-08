//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Futex Implementation - Fast Userspace Mutex
//!
//! # Design
//! - Static waiter pool (spinlock-protected slots) for zero-allocation futex
//! - Static hash table mapping FutexKey → waiter chain (singly-linked list)
//! - All locks use `lock_irqsave()` for interrupt safety
//! - Wake uses `Task::wake_up()` (enqueue + resched) for correct scheduling
//! - Wait inserts into chain then sets INTERRUPTIBLE under lock to prevent lost wakeup
//!
//! # Futex key semantics (Linux parity)
//! - PRIVATE futexes key on (mm identity, uaddr): the second key component is
//!   the address of the task's `AddressSpace` Arc allocation. All CLONE_VM
//!   threads share one Arc → one key; a forked child COWs a NEW AddressSpace
//!   → different key (exactly Linux's `&mm->mm` key). Kernel threads (no mm)
//!   key on 0.
//! - SHARED futexes key on the virtual address only (see R31-9 note; a
//!   physical-frame key needs page pinning infrastructure).

use crate::sync::spinlock::Spinlock;
use core::sync::atomic::AtomicU32;
use crate::process::Task;
use crate::process::task::TaskState;
use crate::syscall::errno::{EINVAL, EFAULT, EAGAIN, ENOSYS, ETIMEDOUT};

/// FUTEX opcodes
pub const FUTEX_WAIT: i32 = 0;
pub const FUTEX_WAKE: i32 = 1;
pub const FUTEX_FD: i32 = 2;
pub const FUTEX_REQUEUE: i32 = 3;
pub const FUTEX_CMP_REQUEUE: i32 = 4;
pub const FUTEX_WAKE_OP: i32 = 5;
pub const FUTEX_LOCK_PI: i32 = 6;
pub const FUTEX_UNLOCK_PI: i32 = 7;
pub const FUTEX_TRYLOCK_PI: i32 = 8;
pub const FUTEX_WAIT_BITSET: i32 = 9;
pub const FUTEX_WAKE_BITSET: i32 = 10;
pub const FUTEX_WAIT_REQUEUE_PI: i32 = 11;
pub const FUTEX_CMP_REQUEUE_PI: i32 = 12;
pub const FUTEX_LOCK_PI2: i32 = 13;

pub const FUTEX_PRIVATE_FLAG: i32 = 128;
pub const FUTEX_CLOCK_REALTIME: i32 = 256;
pub const FUTEX_CMD_MASK: i32 = !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);

// sc184 (session R, screen gate): devmgr frozen-rwlock birth tracer state
// (see the FXDV block in do_futex). Latched from the binder SAREG tracer
// when hdf_devmgr's AddSystemAbility(SA 5100) is seen.
pub static FXDV_PID: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
static FXDV_N: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
const FXDV_RINGSZ: usize = 24;
static FXDV_ADDR: [core::sync::atomic::AtomicUsize; FXDV_RINGSZ] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; FXDV_RINGSZ];
static FXDV_CNT: [core::sync::atomic::AtomicUsize; FXDV_RINGSZ] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; FXDV_RINGSZ];

/// Latch the hdf_devmgr pid for the FXDV tracer (idempotent; first caller
/// wins — binder.rs calls this when the SA 5100 registration is seen).
pub fn fxdv_latch_devmgr(pid: u32) {
    let prev = FXDV_PID.swap(pid, core::sync::atomic::Ordering::Relaxed);
    if prev != pid {
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
static FXRS_TGID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static FXRS_N: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
/// 51st session: census-fed word watch list. When the FUX census prints a
/// stuck group, its uaddr lands here and the FXRS tracer follows every
/// futex op on that word from ANY process — the WAIT/WAKE lifecycle of
/// the frozen lock decides "holder never releases" (no WAKE ever) from
/// "kernel lost the wake" (WAKEs seen, waiter still parked).
pub static FXRS_WORDS: [core::sync::atomic::AtomicUsize; 16] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; 16];

pub fn fxrs_word_add(uaddr: usize) {
    if uaddr == 0 {
        return;
    }
    for w in FXRS_WORDS.iter() {
        let cur = w.load(core::sync::atomic::Ordering::Relaxed);
        if cur == uaddr {
            return;
        }
        if cur == 0
            && w.compare_exchange(
                0,
                uaddr,
                core::sync::atomic::Ordering::AcqRel,
                core::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
        {
            return;
        }
    }
}

#[inline]
pub fn fxrs_word_watched(uaddr: usize) -> bool {
    for w in FXRS_WORDS.iter() {
        if w.load(core::sync::atomic::Ordering::Relaxed) == uaddr {
            return true;
        }
    }
    false
}

// sc58 (58th session): the wake-side verdict ring. Every wedge census so
// far shows the same shape on the stuck words — anywoken=0 and (FXRS)
// total futex-syscall silence AFTER the word went stuck — but FXRS only
// arms once the census prints a word (park age >= 3 s), so the decisive
// moment, the park's BIRTH, is unobserved: the wake that was lost (or
// never sent) happened in that gap. This ring records every WAKE-class
// futex syscall (WAKE / WAKE_BITSET / WAKE_OP / CMP_REQUEUE) system-wide
// from t=0 with its return value; the stuck census then dumps the window
// around the OLDEST stuck parks' birth (park_j-2s .. park_j+5s):
//   - a WAKE on the word with ret>=1 while the waiter never returned
//     names a kernel wake drop (delivery bug);
//   - a WAKE with ret==0 while the waiter WAS linked at that jiffy
//     names a wake-scan miss;
//   - no WAKE near the birth at all names the never-sent branch
//     (userspace ordering — reference-diff territory).
// dfx=fuxstuck-gated push (one lock+copy per wake op); zero cost off.
#[derive(Clone, Copy)]
pub struct FuxWakeOp {
    pub j: u64,
    pub tid: u32,
    pub tgid: u32,
    pub uaddr: u64,
    pub op: u8, // raw FUTEX cmd byte (FUTEX_WAKE=1 etc.)
    pub ret: i32,
}
const FUXW_RING_SZ: usize = 16384; // per CPU: ~6 s of coverage at the OH boot's ~10k wake syscalls/s
const FUXW_NRINGS: usize = 4; // cpu_id % 4 — collision-free on <= 4-CPU boots
const FUXW_ZERO: FuxWakeOp = FuxWakeOp {
    j: 0,
    tid: 0,
    tgid: 0,
    uaddr: 0,
    op: 0,
    ret: 0,
};
pub static FUXW_RINGS: [Spinlock<(usize, [FuxWakeOp; FUXW_RING_SZ])>; FUXW_NRINGS] = [
    Spinlock::new((0, [FUXW_ZERO; FUXW_RING_SZ])),
    Spinlock::new((0, [FUXW_ZERO; FUXW_RING_SZ])),
    Spinlock::new((0, [FUXW_ZERO; FUXW_RING_SZ])),
    Spinlock::new((0, [FUXW_ZERO; FUXW_RING_SZ])),
];

/// Record one completed WAKE-class futex syscall (sc58). Called from
/// do_futex after the op returned. Per-CPU ring: the push must not
/// serialize wake syscalls across CPUs — the wedge it measures is
/// timing-sensitive, and a global lock here changed the boot's shape
/// (ohc74 booted past appfwk.ready with it, the only such boot in the
/// campaign).
pub fn fuxw_push(cmd: u8, uaddr: u64, ret: i32) {
    if !crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::FuxStuck) {
        return;
    }
    let (tid, tgid) = match crate::sched::current() {
        Some(t) => unsafe { ((*t).pid(), (*t).tgid() as u32) },
        None => (0, 0),
    };
    let r = crate::arch::cpu_id() as usize % FUXW_NRINGS;
    let mut g = FUXW_RINGS[r].lock();
    let (idx, arr) = &mut *g;
    arr[*idx] = FuxWakeOp {
        j: crate::drivers::timer::get_jiffies(),
        tid,
        tgid,
        uaddr,
        op: cmd,
        ret,
    };
    *idx = (*idx + 1) % FUXW_RING_SZ;
}

// sc58: waiter-pool occupancy maintained at alloc/free (the census reads
// it O(1) — a census-time sweep of 8192 irqsave locks ran in the timer
// tick and was its own timing pollution); jiffies of the last pool
// exhaustion (alloc_waiter None).
pub static FUX_POOL_USED: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
pub static FUX_POOL_FULL_J: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
// sc209 postmortem: 600 ops died inside RS's own init (EGL/composer
// setup is hundreds of condvar broadcasts), two dozen ops short of the
// HGM window. 4000 ops (~8000 short lines, ~1 min of UART across a
// 45 min boot) covers init + the PostSyncTask park window + any later
// render-loop ops of interest.
const FXRS_BUDGET: usize = 4000;

/// Latch the render_service tgid for the FXRS tracer (idempotent; called
/// from the exec path when /system/bin/render_service loads).
pub fn fxrs_latch(tgid: u32) {
    let prev = FXRS_TGID.swap(tgid, core::sync::atomic::Ordering::Relaxed);
    if prev != tgid {
        crate::pr_warn!("FXRS latch render_service tgid={}", tgid);
    }
}

// sc185 RWFRZ-WATCH: words named by RWFRZ-PARK (writers parked on
// 0x80000000|N readers), watched for their post-park futex lifecycle from
// ANY process. A later WAKE on the word that the parked writer never
// consumes = kernel wake loss; silence forever = readers never released
// (userspace). See the watch block in do_futex and the registration in the
// RWFRZ-PARK hook.
const RWFRZ_WATCH_SZ: usize = 8;
static RWFRZ_WATCH: [core::sync::atomic::AtomicUsize; RWFRZ_WATCH_SZ] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; RWFRZ_WATCH_SZ];
static RWFRZ_WATCH_CNT: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
// sc186: 64 lines died on arena churn before the wedge window; 192 keeps
// the same worst-case UART bound as one parkdump stack dump.
const RWFRZ_WATCH_BUDGET: usize = 192;

// FXDV2: param-arena futex tracer budget (see the FXDV2 block in
// do_futex — traces every futex op on the fixed-address /dev/__parameters__
// window from any process).
static FXDV2_N: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);


pub const FUTEX_BITSET_MATCH_ANY: u32 = 0xffffffff;

// Internal flags
pub const FLAGS_SHARED: u32 = 0x0010;
pub const FLAGS_CLOCKRT: u32 = 0x0020;

/// FUTEX_WAKE_OP encoding (Linux include/uapi/linux/futex.h)
pub const FUTEX_OP_SET: u32 = 0;
pub const FUTEX_OP_ADD: u32 = 1;
pub const FUTEX_OP_OR: u32 = 2;
pub const FUTEX_OP_ANDN: u32 = 3;
pub const FUTEX_OP_XOR: u32 = 4;
pub const FUTEX_OP_CMP_EQ: u32 = 0;
pub const FUTEX_OP_CMP_NE: u32 = 1;
pub const FUTEX_OP_CMP_LT: u32 = 2;
pub const FUTEX_OP_CMP_LE: u32 = 3;
pub const FUTEX_OP_CMP_GT: u32 = 4;
pub const FUTEX_OP_CMP_GE: u32 = 5;

/// Futex key - uniquely identifies a futex
#[derive(Clone, Copy, Debug)]
pub struct FutexKey {
    /// Userspace address
    pub uaddr: usize,
    /// Address-space identity for private futexes (Linux: mm pointer).
    ///
    /// This is the Arc allocation address of the task's AddressSpace —
    /// shared by all CLONE_VM threads of one process, distinct across
    /// processes and across fork(). 0 for kernel threads / mm-less tasks.
    pub mm: usize,
    /// Flags
    pub flags: u32,
    /// PHYSICAL word identity — (page frame << 12) | byte offset —
    /// resolved from the caller's page tables at key construction.
    /// 0 when unresolved (kernel threads, torn-down exit paths, unit
    /// tests). Shared-futex equality is physical, not virtual: the
    /// reference kernel keys shared futexes on (inode, page, offset),
    /// so a wake from ANY mapping of the same word — including a fresh
    /// mmap at a DIFFERENT virtual address after fork+exec (LTP's
    /// tst_checkpoint protocol: parent waits on the MAP_SHARED results
    /// page, the exec'd child re-opens + re-mmaps and wakes) — reaches
    /// the waiter. Rux's MAP_SHARED file pages are single cache frames
    /// aliased in every mapper, so the frame number IS the identity.
    pub phys: usize,
}

/// Resolve the physical word identity of `uaddr` in the CURRENT task's
/// page tables: (frame << 12) | (uaddr & 0xfff). Returns 0 when the walk
/// finds no present user leaf AND a read fault cannot materialize one
/// (callers fall back to uaddr keying — the pre-fix behavior, never
/// worse).
///
/// Fault-in parity with the reference get_futex_key: its
/// get_user_pages_fast FAULTS THE PAGE IN when the PTE is absent. That
/// matters on the WAKE side: FUTEX_WAKE never touches the word in
/// userspace, so a lazy mapping (e.g. the exec'd tst_reinit child that
/// only WAKEs — LTP setpgid03) has NO PTE at syscall entry. Without the
/// fault-in the waker resolves phys=0, hashes the uaddr bucket, and
/// never sees the phys-bucketed waiter (FXK hunt r23: parent
/// phys=0x7ffaa01c vs child phys=0, 10s of 1ms retry wakes, all lost).
fn resolve_futex_phys(uaddr: usize) -> usize {
    let Some(task) = crate::sched::current() else {
        return 0;
    };

    let walk = |u: usize| -> usize {
        // SAFETY: read-only AddressSpace accessor on the current task.
        let root = match unsafe { (*task).address_space() } {
            Some(mm) => mm.root_ppn(),
            None => return 0,
        };
        // SAFETY: read-only page-table walk of the current address space.
        match unsafe { crate::arch::mm::mm_ops::PageTableWalker::walk(root, u as u64) } {
            Some((ppn, bits)) => {
                // Present + user leaf only (device-window leaves and kernel
                // mappings are not futex words).
                if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 {
                    ((ppn as usize) << 12) | (u & 0xfff)
                } else {
                    0
                }
            }
            None => 0,
        }
    };

    let phys = walk(uaddr);
    if phys != 0 {
        return phys;
    }

    // No present user leaf: fault the page in (read access) like the
    // reference's get_user_pages_fast, then re-walk. resolve_futex_phys
    // runs at do_futex entry — BEFORE any futex bucket lock is held and
    // in the caller's own syscall context, so a sleeping fault (file I/O)
    // is legal here. Any fault outcome is fine: the re-walk decides
    // (a still-absent leaf just falls back to uaddr keying).
    // SAFETY: AddressSpace accessor; handle_mm_fault operates on the
    // current task's address space, same as the trap path.
    unsafe {
        if let Some(mm) = (*task).address_space() {
            let page = uaddr & !0xfff;
            let _ = crate::arch::mm::handle_mm_fault(
                &mm,
                crate::arch::mm::VirtAddr::new(page as u64),
                crate::arch::mm::FaultFlags::READ,
            );
        }
    }
    walk(uaddr)
}

impl FutexKey {
    pub fn new(uaddr: usize, mm: usize, flags: u32) -> Self {
        Self { uaddr, mm, flags, phys: 0 }
    }

    /// Production constructor: resolves the physical word identity from
    /// the current page tables (see `phys`).
    pub fn new_user(uaddr: usize, mm: usize, flags: u32) -> Self {
        let k = Self { uaddr, mm, flags, phys: resolve_futex_phys(uaddr) };
        // FXK (r23 setpgid03 hunt): name every futex key construction —
        // uaddr AND the resolved physical identity — so a lost cross-
        // process wake (parent WAIT vs exec'd child WAKE on a MAP_SHARED
        // page) shows which side resolved which frame. Gated behind
        // dfx=futextrace (console-cost discipline; see FXDV2 note).
        if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::FutexTrace) {
            static FXK_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let n = FXK_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < 400 {
                let pid = crate::sched::current()
                    .map(|t| unsafe { (*t).pid() })
                    .unwrap_or(0);
                let comm = crate::sched::current()
                    .map(|t| unsafe {
                        alloc::string::String::from((*t).comm_str())
                    })
                    .unwrap_or_else(|| alloc::string::String::from("?"));
                crate::pr_warn!(
                    "FXK #{} pid={}({}) uaddr={:#x} phys={:#x} shared={}",
                    n,
                    pid,
                    comm,
                    uaddr,
                    k.phys,
                    k.flags & FLAGS_SHARED != 0
                );
            }
        }
        k
    }

    /// Check if two keys match
    pub fn matches(&self, other: &FutexKey) -> bool {
        if (self.flags & FLAGS_SHARED != 0) && self.phys != 0 && other.phys != 0 {
            // Shared waiter: physical-word identity (any mapping of the
            // same word matches — cross-address after exec/fresh mmap).
            // Also covers the musl cleartid interop (shared waiter vs
            // private waker on the SAME word) strictly more precisely
            // than the old uaddr equality.
            self.phys == other.phys
        } else if !(self.flags & FLAGS_SHARED != 0) {
            self.uaddr == other.uaddr && self.mm == other.mm
        } else {
            self.uaddr == other.uaddr
        }
    }
}

/// Derive the private-futex key identity for a task: the Arc allocation
/// address of its AddressSpace (CLONE_VM threads share it; fork does not).
/// Kernel threads without an mm key on 0.
pub fn task_futex_mm_id(task: *const Task) -> usize {
    if task.is_null() {
        return 0;
    }
    // SAFETY: caller guarantees a valid Task; address_space_arc() clones the
    // Arc (bumping the refcount) and we drop it after taking the pointer.
    unsafe {
        match (*task).address_space_arc() {
            Some(arc) => {
                let id = alloc::sync::Arc::as_ptr(&arc) as usize;
                drop(arc);
                id
            }
            None => 0,
        }
    }
}

fn current_futex_mm_id() -> usize {
    match crate::sched::current() {
        // SAFETY: sched::current() returns the current task's raw pointer,
        // valid for the duration of this syscall.
        Some(t) => task_futex_mm_id(t),
        None => 0,
    }
}

/// DFX futex protocol tracer (feature `dfx-futex-trace`).
#[cfg(feature = "dfx-futex-trace")]
fn ftx_trace(tag: &[u8], uaddr: usize, a: u64, b: u64) {
    use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
    let pid = crate::sched::current().map(|t| unsafe { (*t).pid() as u64 }).unwrap_or(0);
    taskdump_raw_line(b"FTX ");
    taskdump_raw_line(tag);
    taskdump_raw_line(b" pid=");
    taskdump_dec(pid);
    taskdump_raw_line(b" u=");
    taskdump_dec(uaddr as u64);
    taskdump_raw_line(b" a=");
    taskdump_dec(a);
    taskdump_raw_line(b" b=");
    taskdump_dec(b);
    taskdump_raw_line(b"\n");
}

#[cfg(feature = "dfx-futex-trace")]
fn ftx_opname(op: i32) -> &'static [u8] {
    match op & FUTEX_CMD_MASK {
        FUTEX_WAIT => b"WAIT",
        FUTEX_WAKE => b"WAKE",
        FUTEX_WAIT_BITSET => b"WAITBS",
        FUTEX_WAKE_BITSET => b"WAKEBS",
        FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => b"REQUEUE",
        FUTEX_WAKE_OP => b"WAKEOP",
        _ => b"OTHER",
    }
}

#[cfg(not(feature = "dfx-futex-trace"))]
fn ftx_trace(_tag: &[u8], _uaddr: usize, _a: u64, _b: u64) {}

/// Waiter information
struct Waiter {
    /// Futex key
    key: FutexKey,
    /// Waiting task
    task: *mut Task,
    /// bitset
    bitset: u32,
    /// Whether already woken
    woken: bool,
    /// Jiffies when this waiter was linked into its bucket chain (the
    /// park stamp for the dfx=fuxstuck census; 0 on the alloc_waiter
    /// placeholder, overwritten at real insertion). NOT reset on requeue —
    /// the park episode continues, so the original stamp stays truthful.
    park_j: u64,
    /// Hash bucket this waiter is currently linked in. Tracked per-waiter
    /// because FUTEX_REQUEUE can move it to uaddr2's bucket while it sleeps;
    /// a stale bucket at removal time silently leaked the slot (review 6.1).
    bucket: usize,
    /// Next waiter in hash chain
    next: Option<usize>,
}

// SAFETY: Waiter is only ever accessed while the per-slot Spinlock is held,
// serialising all reads and writes.  The `task` raw pointer is only
// dereferenced by the waker after validating it is non-null.
unsafe impl Send for Waiter {}
unsafe impl Sync for Waiter {}

/// Waiter pool size - from config
const WAITER_POOL_SIZE: usize = crate::config::FUTEX_WAITER_POOL_SIZE;

/// Waiter pool. D-11 keyed (WaiterSlotKey, slice 3): slot locks taken
/// under a bucket guard go through `lock_nested(&bucket.proof())` (the
/// wake chain walk and the diagnostic scan do); the top-level
/// allocator and free paths stay plain. The requeue relink window's
/// same-class double-slot holds stay plain by design.
static WAITER_POOL: [crate::sync::lockkey::KeyedSpinlock<
    crate::sync::lockkey::WaiterSlotKey,
    Option<Waiter>,
>; WAITER_POOL_SIZE] = {
    const INIT: crate::sync::lockkey::KeyedSpinlock<
        crate::sync::lockkey::WaiterSlotKey,
        Option<Waiter>,
    > = crate::sync::lockkey::KeyedSpinlock::new(None);
    [INIT; WAITER_POOL_SIZE]
};

/// Hash bucket count - from config
const HASH_SIZE: usize = crate::config::FUTEX_HASH_SIZE;

/// Waiter list head for each bucket. D-11 keyed (FutexBucketKey): the
/// guard's `proof()` is what the nested GRQ acquire in the wake path
/// requires — the bucket->GRQ nesting of INV-LOCK-5 (R12-2) is a
/// compile fact. NOTE: FUTEX_REQUEUE holds TWO buckets (lo/hi, index
/// ordered) via plain lock_irqsave — a same-class ordered double
/// acquire, deliberately outside the keyed-nesting API (which exists
/// to fence ACROSS classes).
///
/// 62nd (ohw27/ohw28): the chain is FIFO (tail append) — reference
/// plist parity. Linux's futex queues equal-priority waiters FIFO
/// (lib/plist.c plist_add walks FORWARD past equal priorities and
/// list_add_tails before the first higher-prio node), so the OLDEST
/// waiter receives the first wake of every burst. Our head-insert made
/// the chain LIFO: on a word with constant transient re-parkers (the
/// samgr FFRT worker family's contended inner mutex), every FUTEX_WAKE
/// served the newest transient and the one long-parked waiter — the
/// thread holding samgr's listenerMapLock_ inside its IPC handler —
/// starved for the whole contention window (ohw27: 33 ret=1 wakes by
/// four other threads, the parked owner's record woken=false to the
/// end). The map lock never released, the M2 waiter pandemic followed,
/// and the boot wedged at the appfwk gate 20+ boots in a row. With
/// FIFO, the parked holder gets the first wake and the wedge cannot
/// form. The tail pointer keeps append O(1) (no hot-path regression;
/// the requeue splice below becomes O(1) too).
#[derive(Clone, Copy)]
pub struct FutexChain {
    pub head: Option<usize>,
    pub tail: Option<usize>,
}

static HASH_HEADS: [crate::sync::lockkey::KeyedSpinlock<
    crate::sync::lockkey::FutexBucketKey,
    FutexChain,
>; HASH_SIZE] = {
    const INIT: crate::sync::lockkey::KeyedSpinlock<
        crate::sync::lockkey::FutexBucketKey,
        FutexChain,
    > = crate::sync::lockkey::KeyedSpinlock::new(FutexChain {
        head: None,
        tail: None,
    });
    [INIT; HASH_SIZE]
};

/// Diagnostic (b133/b134 stall hunt): find the live futex-waiter entry of
/// `task`, if any. Returns (uaddr, key.mm, woken). The binder stall report
/// calls this for txn-holder threads parked in futex_wait: a `woken=true`
/// waiter that never resumed indicts the wake→run path, while `woken=false`
/// after minutes indicts the key match (compare key.mm with the task's own
/// current identity) or the userspace releaser.
pub fn waiter_of(task: *mut Task) -> Option<(usize, usize, bool)> {
    for b in HASH_HEADS.iter() {
        let h = b.lock_irqsave();
        let mut idx = h.head;
        while let Some(i) = idx {
            // SAFETY: slot indices come from chain links under the bucket
            // lock; slots are freed only under both locks, so the entry we
            // copy out stays live for the duration of the read.
            let (entry, next) = {
                let slot = WAITER_POOL[i].lock_nested(&h.proof());
                match *slot {
                    Some(ref w) => (
                        Some((w.key.uaddr, w.key.mm, w.woken, core::ptr::eq(w.task, task))),
                        w.next,
                    ),
                    None => (None, None),
                }
            };
            match entry {
                Some((uaddr, mm, woken, true)) => {
                    drop(h);
                    return Some((uaddr, mm, woken));
                }
                Some(_) => {
                    idx = next;
                }
                None => break,
            }
        }
        drop(h);
    }
    None
}

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
                        let t = unsafe { (*w.task).pid() };
                        let live = core::ptr::eq(
                            // SAFETY: diagnostic pid-hash lookup.
                            unsafe { crate::sched::find_task_by_pid(t) },
                            w.task,
                        );
                        let hit = live && (unsafe { (*w.task).tgid() as u32 } == tgid);
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
/// park does not decode. dfx-gated by the caller; walker-only reads
/// of the pinned waiter's frozen mm.
pub fn ffrt_chain_walk(
    task: *mut crate::process::Task,
    uaddr: usize,
) -> Option<(alloc::string::String, Option<u64>)> {
    use core::fmt::Write as _;
    if task.is_null() {
        return None;
    }
    // SAFETY: diagnostic reads of the parked task under the
    // for_each/try contract; the task is pinned by the caller's pid
    // lookup and parked in the futex this walk decodes.
    let tk = unsafe { &*task };
    let top = tk.kernel_stack_top_checked()?;
    #[cfg(feature = "x86_64")]
    let usp = unsafe {
        (*(top.wrapping_sub(crate::arch::x86_64::pt_regs::PT_REGS_SIZE)
            as *const crate::arch::x86_64::pt_regs::PtRegs))
        .rsp as u64
    };
    #[cfg(feature = "riscv64")]
    let usp = unsafe {
        (*(top.wrapping_sub(crate::arch::riscv64::pt_regs::PT_REGS_SIZE)
            as *const crate::arch::riscv64::pt_regs::PtRegs))
        .sp as u64
    };
    #[cfg(feature = "aarch64")]
    let usp = unsafe {
        (*(top.wrapping_sub(crate::arch::aarch64::pt_regs::PT_REGS_SIZE)
            as *const crate::arch::aarch64::pt_regs::PtRegs))
        .sp as u64
    };
    if usp == 0 {
        return None;
    }
    let aspace = tk.address_space()?;
    let root = aspace.root_ppn();
    let mgr = aspace.vma_try_read()?;
    let rd = |va: u64| -> Option<u64> {
        // SAFETY: read-only page-table walk of the parked task's mm.
        match unsafe { crate::arch::mm::mm_ops::PageTableWalker::walk(root, va) } {
            Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
                let kv = crate::arch::mm::memory_layout::phys_to_virt(
                    crate::arch::mm::memory_layout::PhysAddr::new((ppn << 12) | (va & 0xfff)),
                );
                // SAFETY: linear-mapped RAM word.
                Some(unsafe { core::ptr::read_volatile(kv.as_usize() as *const u64) })
            }
            _ => None,
        }
    };
    let heapish = |va: u64| -> bool {
        if va <= 0x10000 || (va >> 47) != 0 {
            return false;
        }
        match mgr.find(crate::mm::vma::VirtAddr::new(va as usize)) {
            Some(vma) => {
                !vma.flags().is_executable()
                    && !vma.flags().contains(crate::mm::vma::VmaFlags::GROWSDOWN)
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
                match mgr.find(crate::mm::vma::VirtAddr::new(va as usize)) {
                    Some(vma) => {
                        !vma.flags().is_executable()
                            && matches!(
                                vma.vma_type(),
                                crate::mm::vma::VmaType::FileBacked
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
                match mgr.find(crate::mm::vma::VirtAddr::new(va as usize)) {
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
                        crate::ipc::binder::binder_sareg_samgr_pid();
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
/// frozen mm; dfx-gated by the caller. Returns None when nothing in
/// the window shape-tests (plain cond/mutex parks).
pub fn ffrt_mutex_scan(task: *mut crate::process::Task) -> Option<alloc::string::String> {
    use core::fmt::Write as _;
    if task.is_null() {
        return None;
    }
    // SAFETY: diagnostic reads of the parked task under the
    // for_each/try contract; the task is pinned by the caller's pid
    // lookup and parked in the futex this scan decodes.
    let tk = unsafe { &*task };
    let top = tk.kernel_stack_top_checked()?;
    #[cfg(feature = "x86_64")]
    let usp = unsafe {
        (*(top.wrapping_sub(crate::arch::x86_64::pt_regs::PT_REGS_SIZE)
            as *const crate::arch::x86_64::pt_regs::PtRegs))
        .rsp as u64
    };
    #[cfg(feature = "riscv64")]
    let usp = unsafe {
        (*(top.wrapping_sub(crate::arch::riscv64::pt_regs::PT_REGS_SIZE)
            as *const crate::arch::riscv64::pt_regs::PtRegs))
        .sp as u64
    };
    #[cfg(feature = "aarch64")]
    let usp = unsafe {
        (*(top.wrapping_sub(crate::arch::aarch64::pt_regs::PT_REGS_SIZE)
            as *const crate::arch::aarch64::pt_regs::PtRegs))
        .sp as u64
    };
    if usp == 0 {
        return None;
    }
    let aspace = tk.address_space()?;
    let root = aspace.root_ppn();
    let mgr = aspace.vma_try_read()?;
    let rd = |va: u64| -> Option<u64> {
        // SAFETY: read-only page-table walk of the parked task's mm.
        match unsafe { crate::arch::mm::mm_ops::PageTableWalker::walk(root, va) } {
            Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
                let kv = crate::arch::mm::memory_layout::phys_to_virt(
                    crate::arch::mm::memory_layout::PhysAddr::new((ppn << 12) | (va & 0xfff)),
                );
                // SAFETY: linear-mapped RAM word.
                Some(unsafe { core::ptr::read_volatile(kv.as_usize() as *const u64) })
            }
            _ => None,
        }
    };
    let heapish = |va: u64| -> bool {
        if va <= 0x10000 || (va >> 47) != 0 {
            return false;
        }
        match mgr.find(crate::mm::vma::VirtAddr::new(va as usize)) {
            Some(vma) => {
                !vma.flags().is_executable()
                    && !vma.flags().contains(crate::mm::vma::VmaFlags::GROWSDOWN)
            }
            None => false,
        }
    };
    let exec_ptr = |va: u64| -> bool {
        match mgr.find(crate::mm::vma::VirtAddr::new(va as usize)) {
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
pub static FUXWATCH_ADDR: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
// sc57b: second watch slot — the chain walk auto-arms it on the first
// NEW shape-detected lock head that differs from FUXWATCH_ADDR's lock
// (the wedge's nested "B" mutex: the FFRT queue-internal lock).
pub static FUXWATCH2_ADDR: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
static FUXWATCH2_HITS: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
static FUXWATCH_DELAY_S: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
static FUXWATCH_HITS: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
// step-marker SET: two CPUs can fault+step concurrently; a single
// marker would lose one thread's #DB → SIGTRAP → thread killed.
static FUXWATCH_STEP_TIDS: [core::sync::atomic::AtomicU32; 4] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 4];
static FUXWATCH_MM_DONE: [core::sync::atomic::AtomicU64; 16] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 16];
const FUXWATCH_BUDGET: usize = 4096;
use core::sync::atomic::Ordering as Relaxed2;

/// 62nd (ohw13 post-decode): owner-of-record latch for the watched
/// lock's l word. Every successful 0->1 acquire write overwrites the
/// latch; after a freeze no further 0->1 writes happen (only old=2
/// pile-ins), so the latch ends up naming the tid whose acquire was
/// the freeze's last one — the holder that never unlocked. The
/// TXN-HANG-BWR audit reads this and force-prints that thread's row
/// (FUX-OWNER) with a deep frame walk, regardless of the census
/// filters that previously hid it (tid344-class rows had no stacked
/// decode and no chain hit).
pub static FUXWATCH_OWNER_TID: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
/// Jiffies of the latched acquire (the latch age, for the row).
pub static FUXWATCH_OWNER_J: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

/// Parse fuxwatch=0xADDR[,DELAYSEC] (and sc58: fuxwatch2=0xADDR) once at
/// boot (called from the dfx init path). Empty/absent leaves the watch
/// off. fuxwatch2 pre-arms the second slot (the census/chain-walk
/// auto-arm only fills it when free).
pub fn fuxwatch_parse() {
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
            crate::pr_warn!("fuxwatch armed target={:#x} delay={}s", a, delay);
            // sc58: the arm rounds run in the censusd kthread, which was
            // only spawned lazily on the first DEFERRED print — ohc73/74
            // never queued one before the wedge window, so the watch
            // words never armed (0 FW lines with FUXWATCH2-AUTO armed).
            // Spawn censusd now (parse runs in early-boot task context).
            crate::dfx::taskdump::ensure_defer_thread();
        }
    }
    fuxwatch2_parse();
}

fn fuxwatch2_parse() {
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
            crate::pr_warn!("fuxwatch2 armed target={:#x}", a);
            // sc58: censusd must exist for the arm rounds (see slot 1).
            crate::dfx::taskdump::ensure_defer_thread();
        }
    }
}

/// Arm round (censusd kthread context): clear W on the watched word's
/// page in every mm that maps it. Re-run cheaply; each mm armed once
/// (the fault/#DB dance keeps W cleared afterwards).
#[cfg(feature = "x86_64")]
pub fn fuxwatch_arm_round() {
    use core::sync::atomic::Ordering::Relaxed;
    let addr = FUXWATCH_ADDR.load(Relaxed);
    let addr2 = FUXWATCH2_ADDR.load(Relaxed);
    if (addr == 0 || FUXWATCH_HITS.load(Relaxed) >= FUXWATCH_BUDGET)
        && (addr2 == 0 || FUXWATCH2_HITS.load(Relaxed) >= FUXWATCH_BUDGET)
    {
        return;
    }
    if addr != 0
        && crate::drivers::timer::get_jiffies() < FUXWATCH_DELAY_S.load(Relaxed) * 100
    {
        return;
    }
    crate::sched::for_each_task_try(|task| {
        // SAFETY: diagnostic read under the for_each_task contract.
        let tk = unsafe { &*task };
        let Some(aspace) = tk.address_space() else {
            return;
        };
        let root = aspace.root_ppn();
        if root == 0 {
            return;
        }
        if FUXWATCH_MM_DONE
            .iter()
            .any(|d| d.load(Relaxed) == root)
        {
            return;
        }
        // page present and writable? (arm both watch words; a page
        // already armed via done-set is skipped whole)
        // SAFETY: read-only walk of the task's page tables.
        let mut to_arm: [u64; 2] = [0; 2];
        let mut narm = 0usize;
        for &wa in [&addr, &addr2] {
            if wa == 0 || to_arm.contains(&wa) {
                continue;
            }
            let ok = unsafe {
                crate::arch::mm::mm_ops::PageTableWalker::walk(root, wa)
            }
            .map(|(_, bits)| bits & 0x2 != 0)
            .unwrap_or(false);
            if ok {
                to_arm[narm] = wa;
                narm += 1;
            }
        }
        if narm == 0 {
            return;
        }
        let g = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock();
        // SAFETY: root is a live user page-table root; R-only heap
        // data pages.
        for &wa in to_arm.iter().take(narm) {
            unsafe {
                crate::arch::mm::mm_ops::mprotect_pte_range(
                    root,
                    wa & !0xfff,
                    1,
                    0x1,
                )
            };
        }
        drop(g);
        crate::arch::x86_64::mm::asid::flush_tlb_all();
        for d in FUXWATCH_MM_DONE.iter() {
            if d
                .compare_exchange(0, root, Relaxed, Relaxed)
                .is_ok()
            {
                break;
            }
        }
        // HOTPATH-PRINT: one line per armed mm (dfx-gated by the param).
        crate::pr_warn!(
            "FUXWATCH-ARM tgid={} root={:#x} page={:#x}",
            tk.tgid(),
            root,
            addr & !0xfff
        );
    });
}

#[cfg(not(feature = "x86_64"))]
pub fn fuxwatch_arm_round() {}

/// Fault-path predicates/helpers (called from the x86_64 fault hook).
#[inline]
pub fn fuxwatch_page_matches(fault_addr: u64) -> bool {
    let a = FUXWATCH_ADDR.load(core::sync::atomic::Ordering::Relaxed);
    let b = FUXWATCH2_ADDR.load(core::sync::atomic::Ordering::Relaxed);
    (a != 0 && (fault_addr & !0xfff) == (a & !0xfff))
        || (b != 0 && (fault_addr & !0xfff) == (b & !0xfff))
}

/// True only for mms WE armed (the done-set) — a fork-COW write fault
/// on the same VA in an un-armed mm must take the normal COW path,
/// never be granted W by the watchpoint.
#[inline]
pub fn fuxwatch_mm_armed(root_ppn: u64) -> bool {
    root_ppn != 0
        && FUXWATCH_MM_DONE
            .iter()
            .any(|d| d.load(core::sync::atomic::Ordering::Relaxed) == root_ppn)
}

#[inline]
pub fn fuxwatch_over_budget() -> bool {
    FUXWATCH_HITS.load(core::sync::atomic::Ordering::Relaxed) >= FUXWATCH_BUDGET
}

/// Log the write and arm the single-step marker; false = step must not
/// be armed (budget). `exact` — the faulting address is the watched
/// WORD itself (cr2 gives byte precision): only those log; same-page
/// neighbours of other heap objects step silently (they must not reach
/// do_page_fault, which would install W and kill the watch).
pub fn fuxwatch_log_write(tid: u32, rip: u64, exact: bool, old_word: u32, fault_addr: u64) -> bool {
    let w2 = FUXWATCH2_ADDR.load(core::sync::atomic::Ordering::Relaxed);
    let claim = |tid: u32| {
        for s in FUXWATCH_STEP_TIDS.iter() {
            if s.load(core::sync::atomic::Ordering::Relaxed) == tid {
                return; // already stepping
            }
        }
        for s in FUXWATCH_STEP_TIDS.iter() {
            if s
                .compare_exchange(0, tid, core::sync::atomic::Ordering::Relaxed, core::sync::atomic::Ordering::Relaxed)
                .is_ok()
            {
                return;
            }
        }
    };
    if !exact {
        claim(tid);
        return true;
    }
    let which2 = w2 != 0 && (fault_addr & !0x3) == (w2 & !0x3);
    let n = FUXWATCH_HITS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if which2 {
        // sc58: slot 2 keeps its own budget accounting (arm_round tests
        // both counters; a shared counter let slot-2 writes retire BOTH
        // slots early).
        FUXWATCH2_HITS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    if n >= FUXWATCH_BUDGET {
        return false;
    }
    // 70th: page-mate writes (the mutex cluster shares the watched page —
    // slot1 armed on the first devmgr waiter word, whose page also carries
    // the DevSvcManager mutex in every observed boot) get their own log
    // arm: the stale-EBUSY founder's acquire (0 -> 0x10, uncontended, no
    // syscall) is otherwise invisible. Budgeted separately.
    let page1 = FUXWATCH_ADDR.load(core::sync::atomic::Ordering::Relaxed) & !0xFFF;
    if !which2 && page1 != 0 && (fault_addr & !0xFFF) == page1 && fault_addr & !0x3 != FUXWATCH_ADDR.load(core::sync::atomic::Ordering::Relaxed) & !0x3 {
        static PG_N: core::sync::atomic::AtomicUsize =
            core::sync::atomic::AtomicUsize::new(0);
        let p = PG_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if p < 900 {
            crate::pr_warn!(
                "FW-PG #{} tid={} addr={:#x} rip={:#x} old={:#x}",
                p,
                tid,
                fault_addr,
                rip,
                old_word
            );
        }
    }
    // HOTPATH-PRINT: compact per-write line; the wedge window carries
    // at most a few hundred. (w2 marker identifies second-watch words
    // when both pages share rips.)
    crate::pr_warn!("FW tid={} rip={:#x} old={} n={} addr={:#x}{}", tid, rip, old_word, n, fault_addr, if which2 { " w2" } else { "" });
    // 62nd: latch the owner-of-record on every successful 0->1
    // acquire-class write (the fast-path CAS or the xchg that wins
    // from UNLOCK). old_word==0 with the word transitioning to a
    // locked value = an acquire. After a freeze only old=2 pile-ins
    // remain, so the latch holds the freeze acquirer.
    if old_word == 0 {
        FUXWATCH_OWNER_TID.store(tid, core::sync::atomic::Ordering::Relaxed);
        FUXWATCH_OWNER_J
            .store(crate::drivers::timer::get_jiffies() as u32, core::sync::atomic::Ordering::Relaxed);
        // oh68: latch the writer's ctx->task at the acquire instant (the
        // only moment a coroutine holder is provably executing); exact =
        // this IS the watched lock word's acquire, not a page-mate write.
        if exact {
            oh68_latch_owner_task(tid);
        }
    }
    claim(tid);
    true
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
                let t = unsafe { (*w.task).pid() };
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
pub fn fuxw_word_shape(task: *mut crate::process::Task, uaddr: u64) -> Option<alloc::string::String> {
    use core::fmt::Write as _;
    if task.is_null() {
        return None;
    }
    // SAFETY: diagnostic reads of the parked task under the caller's
    // pid-lookup pin; stack-top check only.
    let tk = unsafe { &*task };
    if tk.kernel_stack_top_checked().is_none() {
        return None;
    }
    let aspace = tk.address_space()?;
    let root = aspace.root_ppn();
    let mgr = aspace.vma_try_read()?;
    let rd = |va: u64| -> Option<u64> {
        // SAFETY: read-only page-table walk of the parked task's mm.
        match unsafe { crate::arch::mm::mm_ops::PageTableWalker::walk(root, va) } {
            Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
                let kv = crate::arch::mm::memory_layout::phys_to_virt(
                    crate::arch::mm::memory_layout::PhysAddr::new((ppn << 12) | (va & 0xfff)),
                );
                // SAFETY: linear-mapped RAM word.
                Some(unsafe { core::ptr::read_volatile(kv.as_usize() as *const u64) })
            }
            _ => None,
        }
    };
    let filebacked = |va: u64| -> bool {
        match mgr.find(crate::mm::vma::VirtAddr::new(va as usize)) {
            Some(vma) => {
                !vma.flags().is_executable()
                    && matches!(vma.vma_type(), crate::mm::vma::VmaType::FileBacked)
            }
            None => false,
        }
    };
    let exec_ptr = |va: u64| -> bool {
        match mgr.find(crate::mm::vma::VirtAddr::new(va as usize)) {
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
pub fn deep_ufw(task: *mut crate::process::Task) -> Option<alloc::string::String> {
    use core::fmt::Write as _;
    if task.is_null() {
        return None;
    }
    // SAFETY: diagnostic reads of the parked task under the caller's
    // pid-lookup pin; the derefs touch only the stack-top check.
    let tk = unsafe { &*task };
    let top = tk.kernel_stack_top_checked()?;
    #[cfg(feature = "x86_64")]
    let (urbp, ursp) = unsafe {
        let regs = (top.wrapping_sub(crate::arch::x86_64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::x86_64::pt_regs::PtRegs;
        ((*regs).rbp as u64, (*regs).rsp as u64)
    };
    #[cfg(feature = "riscv64")]
    let (urbp, ursp) = unsafe {
        let regs = (top.wrapping_sub(crate::arch::riscv64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::riscv64::pt_regs::PtRegs;
        ((*regs).s0 as u64, (*regs).sp as u64)
    };
    #[cfg(feature = "aarch64")]
    let (urbp, ursp) = unsafe {
        let regs = (top.wrapping_sub(crate::arch::aarch64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::aarch64::pt_regs::PtRegs;
        ((*regs).regs[29] as u64, (*regs).sp as u64)
    };
    if ursp == 0 {
        return None;
    }
    let aspace = tk.address_space()?;
    let root = aspace.root_ppn();
    let rd = |va: u64| -> Option<u64> {
        // SAFETY: read-only page-table walk of the parked task's mm.
        match unsafe { crate::arch::mm::mm_ops::PageTableWalker::walk(root, va) } {
            Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
                let kv = crate::arch::mm::memory_layout::phys_to_virt(
                    crate::arch::mm::memory_layout::PhysAddr::new((ppn << 12) | (va & 0xfff)),
                );
                // SAFETY: linear-mapped RAM word.
                Some(unsafe { core::ptr::read_volatile(kv.as_usize() as *const u64) })
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

/// Read the watched word's CURRENT value through the walker (fault
/// context: the word still holds its pre-write value).
pub fn fuxwatch_read_word(root_ppn: u64) -> u32 {
    let addr = FUXWATCH_ADDR.load(core::sync::atomic::Ordering::Relaxed);
    if addr == 0 {
        return 0xffff_ffff;
    }
    // SAFETY: read-only walk of the current mm.
    match unsafe { crate::arch::mm::mm_ops::PageTableWalker::walk(root_ppn, addr) } {
        Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
            let kv = crate::arch::mm::memory_layout::phys_to_virt(
                crate::arch::mm::memory_layout::PhysAddr::new((ppn << 12) | (addr & 0xfff)),
            );
            // SAFETY: linear-mapped RAM word.
            unsafe { core::ptr::read_volatile(kv.as_usize() as *const u32) }
        }
        _ => 0xffff_ffff,
    }
}

/// #DB hook: true when this trap is our single step (marker consumed).
#[inline]
pub fn fuxwatch_take_step(tid: u32) -> bool {
    for s in FUXWATCH_STEP_TIDS.iter() {
        if s
            .compare_exchange(tid, 0, core::sync::atomic::Ordering::Relaxed, core::sync::atomic::Ordering::Relaxed)
            .is_ok()
        {
            return true;
        }
    }
    false
}

#[inline]
pub fn fuxwatch_addr() -> u64 {
    FUXWATCH_ADDR.load(core::sync::atomic::Ordering::Relaxed)
}

/// sc57 (57th): one-shot region scan — a SUSPENDED FFRT coroutine's
/// stack lives in an 8MB stack-pool region (same class as the worker
/// pthread stacks the chain walk already sees); its frames hold the
/// contested lock pointer mid-stack while no thread does. Sweep the
/// top of every large anonymous region of the tgid's mm for the lock
/// range once per boot per lock; print region+offset hits.
/// 61st: single-word read through a task's page tables (walker-only,
/// diagnostic). Shared by the region scan's M3 shape test.
fn rd_word(root: u64, va: u64) -> Option<u64> {
    match unsafe { crate::arch::mm::mm_ops::PageTableWalker::walk(root, va) } {
        Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
            let kv = crate::arch::mm::memory_layout::phys_to_virt(
                crate::arch::mm::memory_layout::PhysAddr::new((ppn << 12) | (va & 0xfff)),
            );
            // SAFETY: linear-mapped RAM word.
            Some(unsafe { core::ptr::read_volatile(kv.as_usize() as *const u64) })
        }
        _ => None,
    }
}

/// 61st: heap-class pointer test against a task's VMA map (non-exec,
/// non-growdown, canonical userspace). Same rule as the chain walks.
/// `M` is the guard type returned by `vma_try_read` (derefs to the
/// VmaManager).
fn heap_class<M>(va: u64, mgr: &M) -> bool
where
    M: core::ops::Deref<Target = crate::mm::vma::VmaManager>,
{
    if va <= 0x10000 || (va >> 47) != 0 {
        return false;
    }
    match mgr.find(crate::mm::vma::VirtAddr::new(va as usize)) {
        Some(vma) => {
            !vma.flags().is_executable()
                && !vma.flags().contains(crate::mm::vma::VmaFlags::GROWSDOWN)
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
    crate::sched::for_each_task_try(|task| {
        if scanned_regions > 0 {
            return;
        }
        // SAFETY: diagnostic reads under the for_each_task contract.
        let tk = unsafe { &*task };
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
            if !matches!(vma.vma_type(), crate::mm::vma::VmaType::Anonymous)
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
                    match unsafe {
                        crate::arch::mm::mm_ops::PageTableWalker::walk(root, va)
                    } {
                        Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
                            let kv = crate::arch::mm::memory_layout::phys_to_virt(
                                crate::arch::mm::memory_layout::PhysAddr::new(
                                    (ppn << 12) | (va & 0xfff),
                                ),
                            );
                            // SAFETY: linear-mapped RAM word.
                            Some(unsafe {
                                core::ptr::read_volatile(kv.as_usize() as *const u64)
                            })
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
                            match unsafe {
                                crate::arch::mm::mm_ops::PageTableWalker::walk(root, fa)
                            } {
                                Some((ppn2, bits2))
                                    if bits2 & 1 != 0 && bits2 & 0x4 != 0 && ppn2 != 0 =>
                                {
                                    let kv2 = crate::arch::mm::memory_layout::phys_to_virt(
                                        crate::arch::mm::memory_layout::PhysAddr::new(
                                            (ppn2 << 12) | (fa & 0xfff),
                                        ),
                                    );
                                    // SAFETY: linear-mapped RAM word.
                                    Some(unsafe {
                                        core::ptr::read_volatile(kv2.as_usize() as *const u64)
                                    })
                                }
                                _ => None,
                            }
                        })() else {
                            continue;
                        };
                        if matches!(
                            mgr.find(crate::mm::vma::VirtAddr::new(x as usize)),
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
                                            mgr.find(crate::mm::vma::VirtAddr::new(f as usize)),
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

/// sc57 (57th): holder scan — for a named lock-object address (the
/// waiter ring's closing head), scan every same-tgid thread's
/// entry-sp window for the lock pointer (the holder's active
/// lock_guard keeps `&lock` spilled in its frame). Walker-only reads
/// of each parked task's frozen mm; dfx-gated by the caller. Returns
/// "tid+off, ..." candidate list (empty string when none).
/// 61st (ohw19 handoff): ONE-SHOT heap census of every ffrt::mutex
/// object in the tgid's address space. Signature: the mutexPrivate
/// vtable pointer at +0 — a SINGLE runtime VA per boot (read from the
/// KNOWN wedge lock), so every hit is a real ffrt::mutex (ExecuteCtx's
/// vtable differs; the QoS false-positive class is structurally
/// excluded). For each object: l value + waiter ring (up to 6 hops) +
/// the ring members' containing-ctx hint. The lock that is stuck
/// held-with-waiters and is NOT the watched one = L3 (the FFRT-internal
/// lock the M2 holder suspended on) — the cycle bottom the region scan
/// could not reach (the holder's guard lives in a coroutine chunk the
/// chunk-top sweep missed). Once per boot per tgid; the walk over the
/// anon VMAs is the cost (bounded by the same 160-region cap).
/// 61st: the heap-map arm flag — the chain closure stores the known
/// lock's OBJECT base here; the censusd KTHREAD consumes it ONCE and
/// runs the scan in sleepable context. The first cut called the map
/// straight from the TXN-HANG print path (timer/softirq): a full-heap
/// walk with IRQs off — ohw20's console died at 35 s mid-scan.
pub static HEAPMAP_ARM: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

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
    task: *mut crate::process::Task,
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
    let tk = unsafe { &*task };
    let Some(aspace) = tk.address_space() else {
        return alloc::string::String::new();
    };
    let root = aspace.root_ppn();
    let rd = |va: u64| -> Option<u64> {
        // SAFETY: read-only page-table walk of the task's mm.
        match unsafe { crate::arch::mm::mm_ops::PageTableWalker::walk(root, va) } {
            Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
                let kv = crate::arch::mm::memory_layout::phys_to_virt(
                    crate::arch::mm::memory_layout::PhysAddr::new((ppn << 12) | (va & 0xfff)),
                );
                // SAFETY: linear-mapped RAM word.
                Some(unsafe { core::ptr::read_volatile(kv.as_usize() as *const u64) })
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
            .and_then(|f| crate::syscall::misc::oh67_fd_ready(f))
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
    crate::sched::for_each_task_try(|task| {
        // SAFETY: diagnostic read-only walk under the for_each contract.
        let tk = unsafe { &*task };
        if done || tk.tgid() as u32 != tgid {
            return;
        }
        let Some(aspace) = tk.address_space() else {
            return;
        };
        let root = aspace.root_ppn();
        let rd = |va: u64| -> Option<u64> {
            // SAFETY: read-only page-table walk of this task's mm.
            match unsafe { crate::arch::mm::mm_ops::PageTableWalker::walk(root, va) } {
                Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
                    let kv = crate::arch::mm::memory_layout::phys_to_virt(
                        crate::arch::mm::memory_layout::PhysAddr::new(
                            (ppn << 12) | (va & 0xfff),
                        ),
                    );
                    // SAFETY: linear-mapped RAM word.
                    Some(unsafe { core::ptr::read_volatile(kv.as_usize() as *const u64) })
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
// All reads are page-table walks of frozen wedged memory; dfx=fuxstuck
// gates the print sites (the audit burst and the FW write path).
// x86_64-only (the OH image and the fs/TLS offsets are x86_64).

/// g_executeCtxTlsKey VALUE (cached per boot; 0 = unknown).
static OH68_KEY: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
/// ExecuteCtx of the latched acquire writer (0 = none).
pub static OH68_OWNER_CTX: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
/// ctx->task at the acquire write (the holder coroutine/thread task).
pub static FUXWATCH_OWNER_TASK: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
static OH68_ROOT: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

#[cfg(target_arch = "x86_64")]
fn oh68_fs_base(tk: &crate::process::Task) -> u64 {
    tk.thread().fs_base
}
#[cfg(not(target_arch = "x86_64"))]
fn oh68_fs_base(_tk: &crate::process::Task) -> u64 {
    0
}

/// Resolve a thread's ExecuteCtx from its tsd array: the cached key, or a
/// validated scan of slots 1..64 (ctx->tid == the reading thread's tid).
/// Returns the ctx pointer (0 = not found); caches the key on first hit.
#[cfg(target_arch = "x86_64")]
fn oh68_ctx_of(root: u64, fs: u64, tid: u32) -> u64 {
    use core::sync::atomic::Ordering::Relaxed;
    if fs == 0 {
        return 0;
    }
    let Some(tsd) = rd_word(root, fs + 0x88) else {
        return 0;
    };
    if (tsd >> 47) != 0 || tsd < 0x1000 {
        return 0;
    }
    let try_ctx = |c: u64| -> bool {
        c != 0 && (c >> 47) == 0 && c > 0x1000 && matches!(rd_word(root, c + 248), Some(v) if (v & 0xffff_ffff) as u32 == tid)
    };
    let cached = OH68_KEY.load(Relaxed) as u64;
    if cached != 0 {
        if let Some(c) = rd_word(root, tsd + cached * 8) {
            if try_ctx(c) {
                return c;
            }
        }
    }
    for k in 1u64..64 {
        let Some(c) = rd_word(root, tsd + k * 8) else {
            continue;
        };
        if try_ctx(c) {
            OH68_KEY.compare_exchange(
                0,
                k as u32,
                Relaxed,
                Relaxed,
            )
            .ok();
            return c;
        }
    }
    0
}
#[cfg(not(target_arch = "x86_64"))]
fn oh68_ctx_of(_root: u64, _fs: u64, _tid: u32) -> u64 {
    0
}

/// Called from the FW write path on an exact acquire-class write
/// (old==0): the faulting thread is executing the lock acquirer at this
/// instant. Reads its TLS chain and latches ctx->task (the holder) plus
/// the ExecuteCtx (for the audit-side ThreadWait park-word check
/// ctx+224). Bounded: cached key = 3 reads, cold = up to 64.
pub fn oh68_latch_owner_task(tid: u32) {
    use core::sync::atomic::Ordering::Relaxed;
    let _ = tid;
    if cfg!(not(target_arch = "x86_64")) {
        return;
    }
    #[cfg(target_arch = "x86_64")]
    {
        let Some(tsk) = crate::sched::current() else {
            return;
        };
        // SAFETY: current-task accessor in the FW fault path (the same
        // use the trap path itself makes right above this latch's
        // caller).
        let tk = unsafe { &*tsk };
        let Some(aspace) = tk.address_space() else {
            return;
        };
        let root = aspace.root_ppn();
        let fs = oh68_fs_base(tk);
        let ctx = oh68_ctx_of(root, fs, tid);
        if ctx == 0 {
            return;
        }
        OH68_OWNER_CTX.store(ctx, Relaxed);
        if let Some(t) = rd_word(root, ctx + 16) {
            FUXWATCH_OWNER_TASK.store(t, Relaxed);
        }
    }
}

/// oh68: audit-time decode. Walks EVERY samgr thread's TLS chain (worker
/// sweep — a thread-mode holder keeps ctx->task set while parked inside
/// Execute()), decodes the latched acquire-time holder task (coroutine
/// case), and for each CoTask in the M2 waiter ring prints label +
/// coRoutine saved RSP/RIP (the notify tasks' identities). The owner's
/// saved coroutine stack is walked and every word landing in an exec VMA
/// is printed as a FILE OFFSET — offline addr2line against the identical
/// image .so names the exact cv/future frames.
pub fn oh68_holder_decode() -> alloc::string::String {
    use core::fmt::Write as _;
    use core::sync::atomic::Ordering::Relaxed;
    let tgid = OH67_M2_TGID.load(Relaxed);
    if tgid == 0 {
        return alloc::string::String::new();
    }
    let mut out = alloc::string::String::new();
    // Pass 1: any thread's aspace root (shared across the tgid).
    crate::sched::for_each_task_try(|task| {
        if OH68_ROOT.load(Relaxed) != 0 {
            return;
        }
        // SAFETY: diagnostic read under the for_each_task contract.
        let tk = unsafe { &*task };
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
        crate::sched::for_each_task_try(|task| {
            if !exec_vmas.is_empty() || exec_vmas.len() >= 48 {
                return;
            }
            // SAFETY: diagnostic read under the for_each_task contract.
            let tk = unsafe { &*task };
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
    crate::sched::for_each_task_try(|task| {
        if swept >= 24 {
            return;
        }
        // SAFETY: diagnostic read under the for_each_task contract.
        let tk = unsafe { &*task };
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
            " w{}[ctx={:#x} task={:#x} gid={:#x} ty={} st={} ft={}]",
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
                        crate::sched::for_each_task_try(|task| {
                            if named {
                                return;
                            }
                            // SAFETY: diagnostic read under for_each.
                            let tk = unsafe { &*task };
                            if tk.tgid() as u32 != tgid {
                                return;
                            }
                            if let Some(aspace) = tk.address_space() {
                                if let Some(mgr) = aspace.vma_try_read() {
                                    if let Some(v) = mgr.find(crate::mm::vma::VirtAddr::new(
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
                // task->wue (+0x128). Thread-mode parks (entry == some
                // ctx+32) are covered by the sweep.
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
    out
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
    crate::sched::for_each_task_try(|task| {
        // SAFETY: diagnostic read under the for_each_task contract.
        let tk = unsafe { &*task };
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
    crate::sched::for_each_task_try(|task| {
        // SAFETY: walker-only reads of the tgid's frozen mm.
        let tk = unsafe { &*task };
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
            if !matches!(vt, crate::mm::vma::VmaType::Anonymous)
                && !matches!(vt, crate::mm::vma::VmaType::FileBacked)
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
    crate::sched::for_each_task_try(|task| {
        // SAFETY: diagnostic reads under the for_each_task contract.
        let tk = unsafe { &*task };
        if tk.tgid() as u32 != tgid {
            return;
        }
        let Some(top) = tk.kernel_stack_top_checked() else {
            return;
        };
        #[cfg(feature = "x86_64")]
        let usp = unsafe {
            (*(top.wrapping_sub(crate::arch::x86_64::pt_regs::PT_REGS_SIZE)
                as *const crate::arch::x86_64::pt_regs::PtRegs))
            .rsp as u64
        };
        #[cfg(feature = "riscv64")]
        let usp = unsafe {
            (*(top.wrapping_sub(crate::arch::riscv64::pt_regs::PT_REGS_SIZE)
                as *const crate::arch::riscv64::pt_regs::PtRegs))
            .sp as u64
        };
        #[cfg(feature = "aarch64")]
        let usp = unsafe {
            (*(top.wrapping_sub(crate::arch::aarch64::pt_regs::PT_REGS_SIZE)
                as *const crate::arch::aarch64::pt_regs::PtRegs))
            .sp as u64
        };
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
                match unsafe {
                    crate::arch::mm::mm_ops::PageTableWalker::walk(root, usp + 8 * wi)
                } {
                    Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
                        let kv = crate::arch::mm::memory_layout::phys_to_virt(
                            crate::arch::mm::memory_layout::PhysAddr::new(
                                (ppn << 12) | ((usp + 8 * wi) & 0xfff),
                            ),
                        );
                        // SAFETY: linear-mapped RAM word.
                        Some(unsafe { core::ptr::read_volatile(kv.as_usize() as *const u64) })
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
                            match unsafe {
                                crate::arch::mm::mm_ops::PageTableWalker::walk(root2, fa)
                            } {
                                Some((ppn2, bits2))
                                    if bits2 & 1 != 0 && bits2 & 0x4 != 0 && ppn2 != 0 =>
                                {
                                    let kv2 = crate::arch::mm::memory_layout::phys_to_virt(
                                        crate::arch::mm::memory_layout::PhysAddr::new(
                                            (ppn2 << 12) | (fa & 0xfff),
                                        ),
                                    );
                                    // SAFETY: linear-mapped RAM word.
                                    Some(unsafe {
                                        core::ptr::read_volatile(kv2.as_usize() as *const u64)
                                    })
                                }
                                _ => None,
                            }
                        })() else {
                            continue;
                        };
                        if matches!(
                            _mgr2.find(crate::mm::vma::VirtAddr::new(x as usize)),
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

/// dfx=fuxstuck (17th successor): futex-stuck census, called at the ~1 Hz
/// timer heartbeat (same slot as the TXN-HANG audit). The post-creation OH
/// stall web parks threads in userspace-internal condvars (useriam's FFRT
/// executor 248-596 s, distributeddata's DB_TaskExecutor, samgr's Executor)
/// — lostpark already acquitted the kernel wake/park machinery, so the open
/// question is per-condvar: WHAT is parked where, for how long, and is any
/// producer of that condvar still runnable. This census stamps every futex
/// park (Waiter::park_j at chain insertion) and, at most one burst per 5 s,
/// reports every wait >= 3 s as one FUX-STUCK line:
///   - uaddr + mm identity of the condvar (exact, not the param-arena class)
///   - tgid/tid + the stuck thread's entry-frame syscall nr and rip
///   - the owning tgid's thread-state histogram (runnable vs futex-parked
///     vs other-int vs unint) — a tgid with zero runnable threads is
///     deadlocked userspace (no kernel lever); one with runnable threads
///     but a never-woken condvar names the starving producer chain
///     (scheduling/priority territory).
/// Locking: phase A walks the bucket chains under the bucket + nested slot
/// locks (waiter_of discipline) copying POD only; phase B reads task state
/// under for_each_task (pid-hash per-bucket lock, slowtask contract) — the
/// two phases never nest. Bounded: 8 lines per burst, 32 sampled waiters.
/// Zero cost when dfx=fuxstuck is off (one switch load).
pub fn futex_stuck_census() {
    use alloc::vec::Vec;
    use core::sync::atomic::Ordering::Relaxed;

    if !crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::FuxStuck) {
        return;
    }
    let now = crate::drivers::timer::get_jiffies();
    // One burst per 5 s (500 jiffies at HZ=100); first call fires.
    static LAST: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let last = LAST.load(Relaxed);
    if last != 0 && (now as u32).wrapping_sub(last) < 500 {
        return;
    }
    if LAST
        .compare_exchange(last, now as u32, Relaxed, Relaxed)
        .is_err()
    {
        return;
    }
    // Whole-boot line budget (a 25 min boot = ~300 bursts x <= 9 lines).
    static LINES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

    // 59th session: run-queue strand census rides the same burst —
    // pickable-shaped runnable tasks with a frozen exec clock (the
    // ohc74 close() strand class: linked, claim=-1, unexecuted 290 s).
    crate::sched::sched::runq_strand_census(now as u64);

    // 63rd session (mission lever 2): the FFRT group's wake→pick
    // latency profile per burst — quiet-phase vs wedge-phase cost.
    crate::sched::sched::ffrt_wait_census();

    // lb343: 3 s bar (was 10 s). The OH boot cascade's decisive parks are
    // 4-5 s class — init's param-service thread sat 5.06 s between
    // hdf_devmgr's ready-set (5.433s) and init's handle (10.496s, lb342),
    // and the DB_TaskExecutor/DB metronomes poll on 4 s timers — all
    // invisible at the old bar. Diagnostics only; the 8-line burst cap
    // plus the SUM truncation line keep the volume bounded.
    const STUCK_MIN_J: u64 = 3 * 100;
    struct Stuck {
        task: *mut Task,
        uaddr: usize,
        mm: usize,
        age_s: u32,
        woken: bool,
        // Filled by phase B (0 = task not in the pid hash anymore).
        tgid: u32,
        tid: u32,
        sys: u64,
        epc: u64,
        // 48th session (wedge holder hunt): comm of the parked task
        // (OS_IPC_* vs OS_FFRT_* discriminates the samgr-internal park
        // classes), plus the LIVE value of the futex word read through
        // the pinned waiter's own page tables. fval is the ENTRY-time
        // expected value (the wait argument); live==fval after a long
        // park means the userspace event never fired (holder never ran
        // unlock), live!=fval while still parked means the wake-side
        // value change already happened and the waiter slept through
        // it — the direct lost-wake discriminator the 47th's analysis
        // (oh-src run/ffrt-park-analysis.md §2.2) said the census
        // could not make.
        comm: [u8; 16],
        live: u64,
        live_ok: bool,
        // 33rd successor — wait-site map. Futex syscall args from the
        // SAME entry frame (op/val/timeout) plus the user-space wait
        // site: the uaddr's VMA decode (which mapping, offset inside
        // it) and up to 4 user-stack words landing in EXECUTABLE VMAs
        // (candidate return addresses naming the FFRT/samgr call site
        // above the libc wrapper — the entry rip alone is the wrapper
        // for every park and discriminates nothing).
        fop: u64,
        fval: u64,
        fto: u64,
        // 34th successor — the timespec VALUE at fto (tv_sec/tv_nsec
        // read through the same foreign-mm walker). fto!=0 alone does
        // not say the park has a deadline: FFRT-class waiters pass a
        // non-NULL timeout whose value may be effectively infinite.
        // A short value that never fires indicts the timeout arm; a
        // huge value re-points at the missing wake event.
        fto_ok: bool,
        fto_sec: i64,
        fto_nsec: i64,
        uvma_ok: bool,
        uvma_start: usize,
        uvma_end: usize,
        uvma_off: usize,
        uvma_cls: u8, // 'A'non, 'F'ile, 'H'eap, 'S'tack, 'M'shm, 'D'ev
        uvma_fd: i32,
        uvma_voff: usize, // file offset when FileBacked (vma.offset + off)
        uret: [u64; 4],
        uret_n: usize,
        // 34th successor — VMA decode of the STRONGEST return-address
        // candidate (uret[1], the first non-libc one: uret[0] is the
        // wrapper's own internals): its containing VMA start/end/class
        // and, for file-backed, the file offset. Host-side: the file
        // offset symbolizes against the same .so on both kernels (the
        // image is identical) — names the samgr/FFRT call site without
        // needing the guest's ASLR base.
        u2v_ok: bool,
        u2v_start: usize,
        u2v_end: usize,
        u2v_off: usize,
        u2v_cls: u8,
        // sc57 (57th session): FFRT waiter-chain walk. The park word is
        // the musl condvar seq word at ExecuteCtx->wn.cv (WaitUntilEntry
        // +0x88); the intrusive node (LinkedList prev@0/next@8) at wn+0
        // links every waiter of one ffrt::mutexPrivate /
        // SharedMutexPrivate into the OWNER object's list head (mutex:
        // `list` inside the mutex object; smx: wList1/wList2). Walking
        // node.next hops waiter->waiter->head; the hop that leaves the
        // waiter's own stack VMA is the head — the lock object itself.
        // Offsets verified against oh-robot ffrt include/sched/
        // execute_ctx.h (WaitUntilEntry: node 0x00, prev 0x10, next
        // 0x18, task 0x20, wtType 0x28, status 0x2c, hasWaitTime 0x30,
        // tp 0x38, cb 0x40, wl 0x60 (musl pthread_mutex 0x28), cv
        // 0x88); the prev-link mutuality check prunes wrong-guess
        // offsets (a hop only counts when *(hop) == cur).
        chn_task: u64,          // wn+0x20: CPUEUTask* (0 = plain thread)
        chn_wt: u32,            // wn+0x28: 0=NORMAL(mutex) 1=READ 2=WRITE
        chn_node: u64,          // the accepted WaitUntilEntry.node address
        chn_hops: [u64; 8],     // next-direction hops (0 = end)
        chn_hops_ok: u32,       // bit i set = hop i's back-link verified
        chn_prev3: [u64; 3],    // prev-direction hops
        chn_head: u64,          // ring-closing hop = the lock object's list head
        chn_head_win: [u64; 8], // words at chn_head-0x28 .. +0x28
    }
    let mut stuck: Vec<Stuck> = Vec::new();
    let mut total = 0usize;

    // ---- Phase A: bucket chains (POD + task ptr copy only) ----
    'buckets: for b in HASH_HEADS.iter() {
        let h = b.lock_irqsave();
        let mut idx = h.head;
        while let Some(i) = idx {
            // SAFETY: slot index from a chain link held under the bucket
            // lock; the tuple is copied out under the nested slot lock.
            let (entry, next) = {
                let slot = WAITER_POOL[i].lock_nested(&h.proof());
                match *slot {
                    Some(ref w) if !w.task.is_null() && w.park_j != 0 => {
                        let age = now.wrapping_sub(w.park_j);
                        if age >= STUCK_MIN_J {
                            (Some((w.task, w.key.uaddr, w.key.mm, age, w.woken)), w.next)
                        } else {
                            (None, w.next)
                        }
                    }
                    Some(ref w) => (None, w.next),
                    None => (None, None),
                }
            };
            if let Some((task, uaddr, mm, age, woken)) = entry {
                total += 1;
                // 48th session: cap raised 32 -> 96 (and the early-break
                // 64 -> 192 below) — on a wedged boot the system-wide
                // stuck population is 65+, and the first-32 sample by
                // bucket-walk order missed the samgr parks entirely
                // (r41v41: FUX-GRAPH showed 1 of 8 wedge parks; the
                // TXN-HANG rows named 8 futex-parked samgr handlers the
                // census never reached). Prints stay bounded (8 + graph);
                // this only widens the walk's collection window.
                if stuck.len() < 96 {
                    stuck.push(Stuck {
                        task,
                        uaddr,
                        mm,
                        age_s: (age / 100) as u32,
                        woken,
                        tgid: 0,
                        tid: 0,
                        sys: 0,
                        epc: 0,
                        comm: [0; 16],
                        live: 0,
                        live_ok: false,
                        fop: 0,
                        fval: 0,
                        fto: 0,
                        fto_ok: false,
                        fto_sec: 0,
                        fto_nsec: 0,
                        uvma_ok: false,
                        uvma_start: 0,
                        uvma_end: 0,
                        uvma_off: 0,
                        uvma_cls: b'-',
                        uvma_fd: -1,
                        uvma_voff: 0,
                        uret: [0; 4],
                        uret_n: 0,
                        u2v_ok: false,
                        u2v_start: 0,
                        u2v_end: 0,
                        u2v_off: 0,
                        u2v_cls: b'-',
                        chn_task: 0,
                        chn_wt: 0,
                        chn_node: 0,
                        chn_hops: [0; 8],
                        chn_hops_ok: 0,
                        chn_prev3: [0; 3],
                        chn_head: 0,
                        chn_head_win: [0; 8],
                    });
                } else if total > 192 {
                    // Sample cap reached with clear headroom — stop the
                    // walk (the SUM line carries the truncation).
                    drop(h);
                    break 'buckets;
                }
            }
            idx = next;
        }
        drop(h);
    }
    if stuck.is_empty() {
        return;
    }

    // ---- Phase B1: identify the stuck tasks (fill tgid/tid/sys/epc and
    // the wait-site decode) ----
    // lb325 wedge: this census runs at the timer-softirq heartbeat (IF=0);
    // the BLOCKING task walk self-deadlocks against a pid-hash bucket held
    // by an interrupted task-context writer. Try-walk (skipped buckets just
    // leave that stuck entry's tgid/tid fields at their phase-A defaults —
    // the uaddr/park-span data from phase A is already in hand).
    crate::sched::for_each_task_try(|task| {
        for s in stuck.iter_mut() {
            // SAFETY: pointer identity comparison only (no deref of the
            // stored pointer).
            if core::ptr::eq(s.task, task) {
                // SAFETY: diagnostic reads under the for_each_task
                // contract (task pinned in its pid-hash bucket).
                let tk = unsafe { &*task };
                s.tgid = tk.tgid() as u32;
                s.tid = tk.pid();
                s.comm.copy_from_slice(tk.comm());
                let mut usp: u64 = 0;
                if let Some(top) = tk.kernel_stack_top_checked() {
                    #[cfg(feature = "x86_64")]
                    {
                        let regs = (top.wrapping_sub(
                            crate::arch::x86_64::pt_regs::PT_REGS_SIZE,
                        )) as *const crate::arch::x86_64::pt_regs::PtRegs;
                        // SAFETY: entry-frame read of this task's pinned
                        // kernel-stack pages.
                        let r = unsafe { &*regs };
                        s.sys = r.orig_rax as u64;
                        s.epc = r.rip as u64;
                        // futex(uaddr, op, val, timeout, ...): rsi/rdx/r10.
                        s.fop = r.rsi;
                        s.fval = r.rdx;
                        s.fto = r.r10;
                        usp = r.rsp;
                    }
                    #[cfg(feature = "riscv64")]
                    {
                        let regs = (top.wrapping_sub(
                            crate::arch::riscv64::pt_regs::PT_REGS_SIZE,
                        )) as *const crate::arch::riscv64::pt_regs::PtRegs;
                        // SAFETY: entry-frame read of this task's pinned
                        // kernel-stack pages.
                        let r = unsafe { &*regs };
                        s.sys = r.a7 as u64;
                        s.epc = r.epc as u64;
                        s.fop = r.a1;
                        s.fval = r.a2;
                        s.fto = r.a3;
                        usp = r.sp;
                    }
                    #[cfg(feature = "aarch64")]
                    {
                        let regs = (top.wrapping_sub(
                            crate::arch::aarch64::pt_regs::PT_REGS_SIZE,
                        )) as *const crate::arch::aarch64::pt_regs::PtRegs;
                        // SAFETY: entry-frame read of this task's pinned
                        // kernel-stack pages.
                        let r = unsafe { &*regs };
                        s.sys = r.syscallno as u64;
                        s.epc = r.pc as u64;
                        s.fop = r.regs[1];
                        s.fval = r.regs[2];
                        s.fto = r.regs[3];
                        usp = r.sp;
                    }
                }
                // Wait-site decode (33rd successor). The waiter is parked
                // in this very syscall, so its user stack is frozen and
                // its mm cannot vanish while the task is pinned here.
                // VMA lookups under a TRY read lock (softirq context —
                // skip the decode this burst rather than spin IF=0
                // against an mmap/munmap writer).
                if let Some(aspace) = tk.address_space() {
                    if let Some(mgr) = aspace.vma_try_read() {
                        use crate::mm::vma::{VirtAddr, VmaType};
                        // (a) uaddr's mapping
                        if let Some(vma) = mgr.find(VirtAddr::new(s.uaddr)) {
                            s.uvma_ok = true;
                            s.uvma_start = vma.start().as_usize();
                            s.uvma_end = vma.end().as_usize();
                            s.uvma_off = s.uaddr - s.uvma_start;
                            s.uvma_voff = vma.offset() + s.uvma_off;
                            let flags = vma.flags();
                            s.uvma_cls = if flags.contains(
                                crate::mm::vma::VmaFlags::GROWSDOWN,
                            ) {
                                b'S'
                            } else if s.uvma_start == aspace.start_brk() {
                                b'H'
                            } else {
                                match vma.vma_type() {
                                    VmaType::FileBacked => {
                                        s.uvma_fd = vma.file_fd();
                                        b'F'
                                    }
                                    VmaType::SharedMemory => b'M',
                                    VmaType::Device => b'D',
                                    VmaType::Anonymous => b'A',
                                }
                            };
                        }
                        // (a1) LIVE futex word (48th session): the word's
                        // value NOW, through the same walker. 4-byte
                        // read at the (4-aligned) uaddr; a misaligned or
                        // unmapped uaddr just leaves live_ok=false.
                        // Compare against fval (the entry-time expected
                        // value): equal after a long park = the wake
                        // event never fired; different while still
                        // parked = the waiter slept through the value
                        // change (lost-wake class).
                        if s.uvma_ok && s.uaddr <= u64::MAX as usize - 4 {
                            let root = aspace.root_ppn();
                            if let Some((ppn, bits)) = unsafe {
                                crate::arch::mm::mm_ops::PageTableWalker::walk(
                                    root,
                                    s.uaddr as u64,
                                )
                            } {
                                if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 {
                                    let kv = crate::arch::mm::memory_layout::phys_to_virt(
                                        crate::arch::mm::memory_layout::PhysAddr::new(
                                            (ppn << 12) | ((s.uaddr & 0xfff) as u64),
                                        ),
                                    );
                                    // SAFETY: linear-mapped RAM word; the
                                    // waiter parked on it this syscall.
                                    let w32 = unsafe {
                                        core::ptr::read_volatile(kv.as_usize() as *const u32)
                                    };
                                    s.live = w32 as u64;
                                    s.live_ok = true;
                                }
                            }
                        }
                        // (a2) the timespec at fto (34th successor): two
                        // u64 words through the walker (the struct can
                        // straddle a page boundary, so walk each word).
                        // Only when fto is non-NULL and user-addressable;
                        // misaligned/speculative pointers just skip the
                        // decode (fto_ok stays false).
                        if s.fto != 0
                            && s.fto <= u64::MAX - 16
                            && crate::arch::uaccess::access_ok(s.fto as usize, 16)
                        {
                            let root = aspace.root_ppn();
                            let mut words = [0u64; 2];
                            let mut ok = true;
                            for wi in 0..2usize {
                                let va = s.fto + 8 * (wi as u64);
                                // SAFETY: read-only walk of the pinned
                                // waiter's page tables; the timespec was
                                // copied in by futex_parse_timeout before
                                // the park, so the page is resident.
                                match unsafe {
                                    crate::arch::mm::mm_ops::PageTableWalker::walk(root, va)
                                } {
                                    Some((ppn, bits))
                                        if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 =>
                                    {
                                        let kv = crate::arch::mm::memory_layout::phys_to_virt(
                                            crate::arch::mm::memory_layout::PhysAddr::new(
                                                (ppn << 12) | (va & 0xfff),
                                            ),
                                        );
                                        // SAFETY: linear-mapped RAM word.
                                        words[wi] = unsafe {
                                            core::ptr::read_volatile(kv.as_usize() as *const u64)
                                        };
                                    }
                                    _ => {
                                        ok = false;
                                        break;
                                    }
                                }
                            }
                            if ok {
                                s.fto_ok = true;
                                s.fto_sec = words[0] as i64;
                                s.fto_nsec = words[1] as i64;
                            }
                        }
                        // (b) user-stack return-address candidates: scan
                        // the 32 words above the entry sp; a word landing
                        // inside an EXECUTABLE vma is a plausible return
                        // address into userspace text (musl omits frame
                        // pointers, so this is a filtered scan, not a
                        // frame walk). Foreign-mm reads go through the
                        // walker + linear map — no CR3 switch, page is
                        // resident (the thread executed on it).
                        if usp != 0 {
                            let root = aspace.root_ppn();
                            let mut va = (usp + 7) & !7u64;
                            for _ in 0..32 {
                                if s.uret_n >= 4 {
                                    break;
                                }
                                // SAFETY: root_ppn of the pinned task's
                                // live mm; walk is read-only.
                                if let Some((ppn, bits)) = unsafe {
                                    crate::arch::mm::mm_ops::PageTableWalker::walk(
                                        root, va,
                                    )
                                } {
                                    if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 {
                                        let kv = crate::arch::mm::memory_layout::phys_to_virt(
                                            crate::arch::mm::memory_layout::PhysAddr::new(
                                                (ppn << 12) | (va & 0xfff),
                                            ),
                                        );
                                        // SAFETY: linear-mapped RAM word.
                                        let w = unsafe {
                                            core::ptr::read_volatile(kv.as_usize() as *const u64)
                                        };
                                        if let Some(vma) =
                                            mgr.find(VirtAddr::new(w as usize))
                                        {
                                            if vma.flags().is_executable() {
                                                s.uret[s.uret_n] = w;
                                                s.uret_n += 1;
                                            }
                                        }
                                    }
                                }
                                va += 8;
                            }
                        }
                        // (c) VMA decode of uret[1] (34th successor): the
                        // strongest call-site candidate. File-backed ->
                        // host symbolizes uret[1]-start+off against the
                        // same .so from the (identical) system image.
                        if s.uret_n > 1 {
                            if let Some(vma) = mgr.find(VirtAddr::new(s.uret[1] as usize)) {
                                s.u2v_ok = true;
                                s.u2v_start = vma.start().as_usize();
                                s.u2v_end = vma.end().as_usize();
                                s.u2v_off = s.uret[1] as usize - s.u2v_start;
                                let fl = vma.flags();
                                s.u2v_cls = if fl.contains(
                                    crate::mm::vma::VmaFlags::GROWSDOWN,
                                ) {
                                    b'S'
                                } else if s.u2v_start == aspace.start_brk() {
                                    b'H'
                                } else {
                                    match vma.vma_type() {
                                        VmaType::FileBacked => b'F',
                                        VmaType::SharedMemory => b'M',
                                        VmaType::Device => b'D',
                                        VmaType::Anonymous => b'A',
                                    }
                                };
                            }
                        }
                        // (d) sc57: FFRT waiter-chain walk —
                        // IMAGE-CALIBRATED (the 56th's +0x1000 symbol
                        // fix re-rooted the park: ffrt::
                        // mutexPrivate::wait+0x157 after std::
                        // condition_variable::wait, disassembly of the
                        // image libffrt: ctx in rbx with wn.task@ctx+0x40,
                        // wn.wl@ctx+0x90, wn.cv@ctx+0xb8, mutex in r14
                        // with wlock@mutex+0xc). The musl cond wait
                        // parks on the waiter's OWN stack node.state
                        // (node = sp-frame word; solo waiters have NULL
                        // node links), so the word itself does not chain
                        // — but pthread_cond_timedwait SPILLS r14 =
                        // &wn.cv (a heap pointer: ExecuteCtx is new'd)
                        // into its frame. Scan the entry-sp window for
                        // heap-class pointers; for each, test whether
                        // cv-{0x88,0x98} is a linked FFRT LinkedList
                        // node (WaitUntilEntry.node, first member);
                        // then walk the verified ring — the hops are the
                        // OTHER waiters' ctx->wn nodes and the ring's
                        // head lives inside the contested
                        // mutexPrivate/SharedMutexPrivate object. All
                        // reads are page-table walks of the pinned
                        // waiter's frozen mm; wrong candidates fail the
                        // back-link verification and leave zeros.
                        if usp != 0 {
                            let root = aspace.root_ppn();
                            // SAFETY: read-only AddressSpace accessor on
                            // the pinned waiter.
                            let rd = |va: u64| -> Option<u64> {
                                match unsafe {
                                    crate::arch::mm::mm_ops::PageTableWalker::walk(root, va)
                                } {
                                    Some((ppn, bits))
                                        if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 =>
                                    {
                                        let kv = crate::arch::mm::memory_layout::phys_to_virt(
                                            crate::arch::mm::memory_layout::PhysAddr::new(
                                                (ppn << 12) | (va & 0xfff),
                                            ),
                                        );
                                        // SAFETY: linear-mapped RAM word.
                                        Some(unsafe {
                                            core::ptr::read_volatile(kv.as_usize() as *const u64)
                                        })
                                    }
                                    _ => None,
                                }
                            };
                            // is a virtual address inside a writable
                            // non-stack VMA (heap/data — where an
                            // ExecuteCtx or lock object lives)?
                            let heapish = |va: u64| -> bool {
                                if va <= 0x10000 || (va >> 47) != 0 {
                                    return false;
                                }
                                match mgr.find(VirtAddr::new(va as usize)) {
                                    Some(vma) => {
                                        !vma.flags().is_executable()
                                            && !vma
                                                .flags()
                                                .contains(
                                                    crate::mm::vma::VmaFlags::GROWSDOWN,
                                                )
                                    }
                                    None => false,
                                }
                            };
                            'cand: for wi in 0..96u64 {
                                let Some(w) = rd(usp + 8 * wi) else {
                                    continue;
                                };
                                if !heapish(w) {
                                    continue;
                                }
                                // W on the stack may be &wn.cv (r14
                                // spill; cv@wn+0x88 or +0x98 depending
                                // on the WaitEntry packing) or &wn.wl
                                // (mutexPrivate::wait's own spill;
                                // wl@wn+0x60 or +0x70) — try each; the
                                // ring verification prunes the wrong
                                // ones.
                                for &xoff in &[0x60u64, 0x70, 0x88, 0x98] {
                                    let node = w.wrapping_sub(xoff);
                                    if !heapish(node) {
                                        continue;
                                    }
                                    let (Some(prev), Some(next)) = (rd(node), rd(node + 8))
                                    else {
                                        continue;
                                    };
                                    // unlinked LinkedList nodes point at
                                    // themselves; linked ones never do
                                    if prev == 0
                                        || next == 0
                                        || prev == node
                                        || next == node
                                        || !heapish(next)
                                        || !heapish(prev)
                                    {
                                        continue;
                                    }
                                    // verify BOTH directions' back-links
                                    // before trusting the ring
                                    let ok_next = rd(next) == Some(node);
                                    let ok_prev = rd(prev + 8) == Some(node);
                                    if !ok_next && !ok_prev {
                                        continue;
                                    }
                                    // candidate accepted: walk next up to
                                    // 8 hops, window the hop that closes
                                    // the ring (the head — a member of
                                    // the lock object)
                                    s.chn_task = rd(node + 0x20).unwrap_or(0);
                                    s.chn_wt = rd(node + 0x28).unwrap_or(0) as u32;
                                    s.chn_node = node;
                                    let mut cur = node;
                                    let mut prev_hop = node;
                                    for h in 0..8 {
                                        let Some(nxt) = rd(cur + 8) else {
                                            break;
                                        };
                                        if nxt == node {
                                            // ring closed: prev_hop is the
                                            // head (its container is the
                                            // lock object)
                                            if s.chn_head == 0 && prev_hop != node {
                                                s.chn_head = prev_hop;
                                                for (hw, wv) in
                                                    s.chn_head_win.iter_mut().enumerate()
                                                {
                                                    if let Some(x) = rd(
                                                        prev_hop
                                                            .wrapping_sub(0x28)
                                                            + 8 * hw as u64,
                                                    ) {
                                                        *wv = x;
                                                    }
                                                }
                                            }
                                            break;
                                        }
                                        if nxt == 0 || (nxt >> 47) != 0 || !heapish(nxt) {
                                            break;
                                        }
                                        if rd(nxt) == Some(cur) {
                                            s.chn_hops_ok |= 1 << h;
                                        }
                                        s.chn_hops[h] = nxt;
                                        prev_hop = cur;
                                        cur = nxt;
                                    }
                                    if s.chn_hops[0] == 0 {
                                        // next-walk dead-ended: the head
                                        // sits the other way; record the
                                        // prev direction
                                        let mut cur = node;
                                        for h in 0..3 {
                                            let Some(prv) = rd(cur) else {
                                                break;
                                            };
                                            if prv == 0
                                                || prv == node
                                                || (prv >> 47) != 0
                                                || !heapish(prv)
                                            {
                                                break;
                                            }
                                            s.chn_prev3[h] = prv;
                                            cur = prv;
                                        }
                                    }
                                    break 'cand;
                                }
                            }
                        }
                    }
                }
                break;
            }
        }
    });

    // ---- Phase B2: per-tgid thread histograms for the stuck tgids ----
    struct Histo {
        tgid: u32,
        run: u32,
        futex: u32,
        int_other: u32,
        unint: u32,
    }
    let mut histos: Vec<Histo> = Vec::new();
    // Same lb325 discipline as phase B1: try-walk from the heartbeat —
    // a skipped bucket only under-counts that tgid's histogram this burst.
    crate::sched::for_each_task_try(|task| {
        // SAFETY: diagnostic reads under the for_each_task contract.
        let tk = unsafe { &*task };
        let tgid = tk.tgid() as u32;
        if !stuck.iter().any(|s| s.tgid == tgid) {
            return;
        }
        let st_bits = tk.state().bits();
        let run = st_bits == crate::process::task::TaskState::RUNNING
            || tk.on_cpu()
            || tk.sched_entity().is_on_rq();
        let fux = tk.futex_wait_enqueued.load(Relaxed);
        let slot = match histos.iter_mut().find(|h| h.tgid == tgid) {
            Some(h) => h,
            None => {
                if histos.len() >= 16 {
                    return;
                }
                histos.push(Histo {
                    tgid,
                    run: 0,
                    futex: 0,
                    int_other: 0,
                    unint: 0,
                });
                histos.last_mut().unwrap()
            }
        };
        if run {
            slot.run += 1;
        } else if fux {
            slot.futex += 1;
        } else if st_bits & 0x1 != 0 {
            slot.int_other += 1;
        } else if st_bits & 0x2 != 0 {
            slot.unint += 1;
        }
    });

    // ---- Phase C: aggregate by (tgid, uaddr) and print ----
    // Two visibility buckets (lb323 lesson: a pure oldest-first top-8 was
    // monopolised by 155 s-class peripheral parks — bluetooth/audio — and
    // never showed the critical-path park that decided the creation race):
    //   OLDEST 4 groups by max park age  — the long-standing anchors;
    //   FRESH   4 groups by min park age  — the currently-developing stall
    //            (smallest age >= threshold among stuck waiters).
    stuck.sort_by(|a, b| b.age_s.cmp(&a.age_s));
    struct Group {
        tgid: u32,
        uaddr: usize,
        n: usize,
        max_age: u32,
        min_age: u32,
        any_woken: bool,
        rep_tid: u32,
        rep_mm: usize,
        rep_sys: u64,
        rep_epc: u64,
        rep_comm: [u8; 16],
        rep_live: u64,
        rep_live_ok: bool,
        rep_fop: u64,
        rep_fval: u64,
        rep_fto: u64,
        rep_fto_ok: bool,
        rep_fto_sec: i64,
        rep_fto_nsec: i64,
        rep_uvma_ok: bool,
        rep_uvma_start: usize,
        rep_uvma_end: usize,
        rep_uvma_off: usize,
        rep_uvma_cls: u8,
        rep_uvma_fd: i32,
        rep_uvma_voff: usize,
        rep_uret: [u64; 4],
        rep_uret_n: usize,
        rep_u2v_ok: bool,
        rep_u2v_start: usize,
        rep_u2v_end: usize,
        rep_u2v_off: usize,
        rep_u2v_cls: u8,
        rep_chn_task: u64,
        rep_chn_wt: u32,
        rep_chn_node: u64,
        rep_chn_hops: [u64; 8],
        rep_chn_hops_ok: u32,
        rep_chn_prev3: [u64; 3],
        rep_chn_head: u64,
        rep_chn_head_win: [u64; 8],
    }
    let mut groups: Vec<Group> = Vec::new();
    for s in stuck.iter() {
        if s.tgid == 0 {
            continue;
        }
        let existing = groups
            .iter_mut()
            .find(|g| g.tgid == s.tgid && g.uaddr == s.uaddr);
        match existing {
            Some(g) => {
                g.n += 1;
                g.max_age = g.max_age.max(s.age_s);
                g.min_age = g.min_age.min(s.age_s);
                g.any_woken |= s.woken;
            }
            None => {
                // 48th session: 24 -> 48 — a wedged boot carries ~16
                // samgr park groups plus ~20 peripheral ones; the 24 cap
                // could exclude the samgr tail from the FUX-GRAPH pass.
                if groups.len() < 48 {
                    groups.push(Group {
                        tgid: s.tgid,
                        uaddr: s.uaddr,
                        n: 1,
                        max_age: s.age_s,
                        min_age: s.age_s,
                        any_woken: s.woken,
                        rep_tid: s.tid,
                        rep_mm: s.mm,
                        rep_sys: s.sys,
                        rep_epc: s.epc,
                        rep_comm: s.comm,
                        rep_live: s.live,
                        rep_live_ok: s.live_ok,
                        rep_fop: s.fop,
                        rep_fval: s.fval,
                        rep_fto: s.fto,
                        rep_fto_ok: s.fto_ok,
                        rep_fto_sec: s.fto_sec,
                        rep_fto_nsec: s.fto_nsec,
                        rep_uvma_ok: s.uvma_ok,
                        rep_uvma_start: s.uvma_start,
                        rep_uvma_end: s.uvma_end,
                        rep_uvma_off: s.uvma_off,
                        rep_uvma_cls: s.uvma_cls,
                        rep_uvma_fd: s.uvma_fd,
                        rep_uvma_voff: s.uvma_voff,
                        rep_uret: s.uret,
                        rep_uret_n: s.uret_n,
                        rep_u2v_ok: s.u2v_ok,
                        rep_u2v_start: s.u2v_start,
                        rep_u2v_end: s.u2v_end,
                        rep_u2v_off: s.u2v_off,
                        rep_u2v_cls: s.u2v_cls,
                        rep_chn_task: s.chn_task,
                        rep_chn_wt: s.chn_wt,
                        rep_chn_node: s.chn_node,
                        rep_chn_hops: s.chn_hops,
                        rep_chn_hops_ok: s.chn_hops_ok,
                        rep_chn_prev3: s.chn_prev3,
                        rep_chn_head: s.chn_head,
                        rep_chn_head_win: s.chn_head_win,
                    });
                }
            }
        }
    }
    let mut order: Vec<usize> = (0..groups.len()).collect();
    order.sort_by(|&a, &b| groups[b].max_age.cmp(&groups[a].max_age));
    // 77th: the OS_Dfx* periodic pollers (DfxWatchdog 60 s poll,
    // DfxIpcCheck) are always-benign stuck groups that monopolize the
    // 8-line print budget on wedged OH boots (ohw108: stuck_total=193
    // printed=8, all watchdog-class — the samgr processLock parks
    // never surfaced). Deprioritize them in BOTH selectors: skipped in
    // the by-age top-4 and in the fresh fill; a genuinely stuck
    // DfxWatchdog still shows via TXN-HANG (it would hold a txn).
    let is_dfx_poller = |g: &Group| g.rep_comm.starts_with(b"OS_Dfx");
    let mut chosen: Vec<(usize, bool)> = Vec::new(); // (group idx, is_fresh)
    for &gi in order.iter() {
        if chosen.len() >= 4 {
            break;
        }
        if !is_dfx_poller(&groups[gi]) {
            chosen.push((gi, false));
        }
    }
    let mut fresh_order = order.clone();
    fresh_order.sort_by(|&a, &b| groups[a].min_age.cmp(&groups[b].min_age));
    for &gi in fresh_order.iter() {
        if chosen.len() >= 8 {
            break;
        }
        if is_dfx_poller(&groups[gi]) {
            continue;
        }
        if !chosen.iter().any(|(c, _)| *c == gi) {
            chosen.push((gi, true));
        }
    }
    let mut printed = 0usize;
    for (gi, is_fresh) in chosen {
        let g = &groups[gi];
        let h = histos.iter().find(|h| h.tgid == g.tgid);
        // HOTPATH-PRINT: fuxstuck census report — gated by
        // dfx=fuxstuck at futex_stuck_census() entry plus the 5s burst
        // budget and the 1200-line LINES cap; never fires on a plain boot.
        use core::fmt::Write as _;
        let mut vmx = alloc::string::String::new();
        if g.rep_uvma_ok && g.rep_uvma_cls == b'F' {
            let _ = write!(vmx, " fd={} voff={:#x}", g.rep_uvma_fd, g.rep_uvma_voff);
        }
        let mut uret_s = alloc::string::String::new();
        for i in 0..g.rep_uret_n {
            let _ = write!(uret_s, "{:#x} ", g.rep_uret[i]);
        }
        let mut fto_s = alloc::string::String::new();
        if g.rep_fto_ok {
            let _ = write!(fto_s, "={}s+{}ns", g.rep_fto_sec, g.rep_fto_nsec);
        }
        let mut u2v_s = alloc::string::String::new();
        if g.rep_u2v_ok {
            let _ = write!(
                u2v_s,
                " u2v={:#x}+{:#x}{}",
                g.rep_u2v_start,
                g.rep_u2v_off,
                g.rep_u2v_cls as char
            );
        }
        // 48th session: comm + live-word verdict on the print.
        let comm_s = core::str::from_utf8(&g.rep_comm)
            .unwrap_or("")
            .trim_end_matches('\0');
        // fval is the 32-bit syscall arg sign-extended into the saved
        // u64 register slot — compare the low 32 bits only (a negative
        // expected value, e.g. the musl requeue-stamped 0x80000010,
        // otherwise reads as a false LIVE!=fval).
        let live_s = if g.rep_live_ok {
            if g.rep_live == (g.rep_fval & 0xffff_ffff) && g.rep_fval != 0 {
                "live==fval"
            } else {
                "LIVE!=fval"
            }
        } else {
            "live=?"
        };
        crate::pr_warn!(
            "FUX-STUCK{} tgid={} tid={} ({}) uaddr={:#x} mm={:#x} n={} park={}s..{}s anywoken={} sys={} epc={:#x} fop={:#x} fval={:#x} {}={:#x} fto={:#x}{} uvma={:#x}-{:#x} {}+{:#x}{} u2v={} uret={}tgidthr[run={} futex={} int={} unint={}]",
            if is_fresh { "-FRESH" } else { "" },
            g.tgid,
            g.rep_tid,
            comm_s,
            g.uaddr,
            g.rep_mm,
            g.n,
            g.min_age,
            g.max_age,
            g.any_woken as u32,
            g.rep_sys,
            g.rep_epc,
            g.rep_fop,
            g.rep_fval,
            live_s,
            g.rep_live,
            g.rep_fto,
            fto_s,
            g.rep_uvma_start,
            g.rep_uvma_end,
            g.rep_uvma_cls as char,
            g.rep_uvma_off,
            vmx,
            u2v_s,
            uret_s,
            h.map(|h| h.run).unwrap_or(0),
            h.map(|h| h.futex).unwrap_or(0),
            h.map(|h| h.int_other).unwrap_or(0),
            h.map(|h| h.unint).unwrap_or(0),
        );
        // 51st session: follow this word's futex lifecycle from here on
        // (FXRS word watch — the WAIT/WAKE trace decides holder-never-
        // releases from kernel-lost-wake).
        // sc57: the FFRT waiter-chain walk lines (only when the layout
        // check admitted the wn — plain futex parks print nothing).
        if g.rep_chn_hops.iter().any(|&h| h != 0) || g.rep_chn_prev3.iter().any(|&h| h != 0) {
            let mut hops_s = alloc::string::String::new();
            for (h, &a) in g.rep_chn_hops.iter().enumerate() {
                if a == 0 {
                    break;
                }
                let _ = write!(
                    hops_s,
                    " {:#x}{}",
                    a,
                    if g.rep_chn_hops_ok & (1 << h) != 0 { "*" } else { "?" }
                );
            }
            let mut prev_s = alloc::string::String::new();
            for &a in g.rep_chn_prev3.iter() {
                if a == 0 {
                    break;
                }
                let _ = write!(prev_s, " {:#x}", a);
            }
            // HOTPATH-PRINT: FUX-CHAIN — same dfx=fuxstuck gate and
            // burst budget as the FUX-STUCK row it follows; one to
            // three lines per reported group.
            crate::pr_warn!(
                "FUX-CHAIN tid={} node={:#x} task={:#x} wt={} hops:{} prev:{}",
                g.rep_tid,
                g.rep_chn_node,
                g.rep_chn_task,
                g.rep_chn_wt,
                hops_s,
                prev_s
            );
            if g.rep_chn_head != 0 {
                let mut win_s = alloc::string::String::new();
                for &wv in g.rep_chn_head_win.iter() {
                    let _ = write!(win_s, "{:#x} ", wv);
                }
                crate::pr_warn!(
                    "FUX-CHAIN-HEAD tid={} head={:#x} win:{}",
                    g.rep_tid,
                    g.rep_chn_head,
                    win_s
                );
            }
        }
        fxrs_word_add(g.uaddr);
        printed += 1;
        if LINES.fetch_add(1, Relaxed) > 1200 {
            break;
        }
    }
    if total > printed && printed == 8 {
        // HOTPATH-PRINT: census overflow marker — same dfx=fuxstuck gate
        // and 5s burst budget as the FUX-STUCK rows above (8 per burst).
        crate::pr_warn!(
            "FUX-STUCK-SUM stuck_total={} printed=8 (sampled {})",
            total,
            96
        );
    }

    // ---- sc58 (58th): the wake-side verdict pass. Three additions, all
    // under this same 5 s burst:
    //   (a) waiter-pool occupancy + last-exhaustion stamp (a wedge-time
    //       pool-full makes every later FUTEX_WAIT spin in backoff
    //       retries — invisible to the bucket-walk census above);
    //   (b) the FUXW ring birth-window dump: WAKE-class syscalls recorded
    //       around the OLDEST stuck parks' births (park_j-2s .. +5s) —
    //       closes the FXRS arming gap (FXRS starts watching a word only
    //       3+ s after it parked; the lost wake happens at birth);
    //   (c) auto-arm fuxwatch slot 2 on the first stuck FFRT-worker park
    //       word in the registered samgr pid — that word IS the FFRT
    //       queue-internal mutex's musl lock word (fval=0x80000010 =
    //       held|waiters): its write trace decides "unlock happens but
    //       waiter never wakes" (kernel drop) from "no unlock ever"
    //       (holder never runs — reference-diff branch).
    //       60th DISABLED: the park word is a musl cv waiter node on
    //       the waiter's OWN stack — its write trace cannot identify
    //       the mutex owner, and arming it stole the slot (and the
    //      censusd arm rounds) from the chain-walk's LOCK-HEAD arm
    //      (fuxwatch-LOCKHEAD), which is the owner-naming instrument.
    {
        use core::fmt::Write as _;
        // (a) pool stats — O(1) counter (see FUX_POOL_USED).
        let pool_used = FUX_POOL_USED.load(Relaxed);
        let last_full = FUX_POOL_FULL_J.load(Relaxed);
        // (c) DISABLED (60th): see the comment above — the stack-word
        // auto-arm lost the slot race against the chain-walk's
        // LOCK-HEAD arm; kept as a const-gated block for reference.
        const CENSUS_STACK_WORD_ARM: bool = false;
        let samgr_pid = crate::ipc::binder::binder_sareg_samgr_pid();
        if CENSUS_STACK_WORD_ARM && samgr_pid != 0 && FUXWATCH2_ADDR.load(Relaxed) == 0 {
            for g in groups.iter() {
                if g.tgid == samgr_pid && g.rep_comm[..7] == *b"OS_FFRT" {
                    let w2 = g.uaddr as u64;
                    if FUXWATCH2_ADDR
                        .compare_exchange(0, w2, Relaxed, Relaxed)
                        .is_ok()
                    {
                        crate::pr_warn!(
                            "FUXWATCH2-AUTO word={:#x} tid={} park={}s",
                            w2,
                            g.rep_tid,
                            g.max_age
                        );
                    }
                    break;
                }
            }
        }
        // (b cont.) pick the dump targets first (top-3 by max_age plus
        // any samgr FFRT group), then snapshot ONLY matching entries —
        // the ring is 64k entries; copying it whole every 5 s would be
        // a 2 MB alloc churn under its lock.
        static CTX_LINES: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        {
            let mut order2: alloc::vec::Vec<usize> = (0..groups.len()).collect();
            order2.sort_by(|&a, &b| groups[b].max_age.cmp(&groups[a].max_age));
            let mut picks: alloc::vec::Vec<usize> = alloc::vec::Vec::new();
            for &gi in order2.iter().take(3) {
                picks.push(gi);
            }
            for (gi, g) in groups.iter().enumerate() {
                if picks.len() >= 4 {
                    break;
                }
                if !picks.contains(&gi)
                    && g.tgid == samgr_pid
                    && g.rep_comm[..7] == *b"OS_FFRT"
                {
                    picks.push(gi);
                }
            }
            if picks.is_empty() || CTX_LINES.load(Relaxed) > 420 {
                crate::pr_warn!(
                    "FUXW-SUM pool={}/{} last_full_j={} (no dump targets)",
                    pool_used,
                    WAITER_POOL_SIZE,
                    if last_full == 0 { -1i64 } else { last_full as i64 },
                );
            } else {
                // windows + word set for the filter pass
                let wins: alloc::vec::Vec<(u64, u64, u64)> = picks
                    .iter()
                    .map(|&gi| {
                        let g = &groups[gi];
                        let birth = now.saturating_sub(g.max_age as u64 * 100);
                        (
                            birth.saturating_sub(200),
                            birth + 500,
                            g.uaddr as u64,
                        )
                    })
                    .collect();
                let mut oldest_j = u64::MAX;
                let mut newest_j = 0u64;
                let mut ring_n = 0u64;
                let mut snap: alloc::vec::Vec<FuxWakeOp> = alloc::vec::Vec::new();
                for ring in FUXW_RINGS.iter() {
                    let g = ring.lock();
                    let (idx, arr) = &*g;
                    for k in 0..FUXW_RING_SZ {
                        let i = (*idx + k) % FUXW_RING_SZ;
                        let e = arr[i];
                        if e.j == 0 {
                            continue;
                        }
                        ring_n += 1;
                        oldest_j = oldest_j.min(e.j);
                        newest_j = newest_j.max(e.j);
                        if wins
                            .iter()
                            .any(|&(lo, hi, w)| (e.j >= lo && e.j <= hi) || e.uaddr == w)
                        {
                            snap.push(e);
                        }
                    }
                }
                snap.sort_by_key(|e| e.j);
                crate::pr_warn!(
                    "FUXW-SUM pool={}/{} last_full_j={} ring={} oldest={} newest={}",
                    pool_used,
                    WAITER_POOL_SIZE,
                    if last_full == 0 { -1i64 } else { last_full as i64 },
                    ring_n,
                    if oldest_j == u64::MAX { -1i64 } else { oldest_j as i64 },
                    newest_j
                );
                for (pi, &gi) in picks.iter().enumerate() {
                    let g = &groups[gi];
                    let birth = now.saturating_sub(g.max_age as u64 * 100);
                    let (win_lo, win_hi) = (wins[pi].0, wins[pi].1);
                    let in_win: alloc::vec::Vec<&FuxWakeOp> = snap
                        .iter()
                        .filter(|e| e.j >= win_lo && e.j <= win_hi)
                        .collect();
                    let ever_word =
                        snap.iter().filter(|e| e.uaddr == g.uaddr as u64).count();
                    if in_win.is_empty() && ever_word == 0 {
                        continue;
                    }
                    use core::fmt::Write as _;
                    let mut hdr = alloc::string::String::new();
                    let _ = write!(
                        hdr,
                        "FUXW-CTX tid={} ({}) word={:#x} birth_j={} win={}..{} in_win={} ever_word={}",
                        g.rep_tid,
                        core::str::from_utf8(&g.rep_comm).unwrap_or("").trim_end_matches('\0'),
                        g.uaddr,
                        birth,
                        win_lo,
                        win_hi,
                        in_win.len(),
                        ever_word
                    );
                    crate::pr_warn!("{}", hdr);
                    for e in in_win.iter().take(12) {
                        if CTX_LINES.fetch_add(1, Relaxed) > 420 {
                            break;
                        }
                        crate::pr_warn!(
                            "FUXW j={} tid={} tgid={} op={} uaddr={:#x} ret={}",
                            e.j,
                            e.tid,
                            e.tgid,
                            e.op,
                            e.uaddr,
                            e.ret
                        );
                    }
                    CTX_LINES.fetch_add(1, Relaxed);
                }
            }
        }
    }

    // ---- Phase D (48th session): FUX-GRAPH — the complete samgr park
    // graph every 6th burst (~30 s). The 8-line sample above cannot name
    // the listenerMapLock_ holder: the wedge's decisive parks (FFRT
    // workers in the samgr pid, park classes WorkerIdleAction /
    // mutexPrivate::wait / BlockUntilThreadAvailable) are drowned by
    // peripheral parks. This pass prints EVERY stuck group whose tgid
    // is the registered samgr pid, with comm and the live-word verdict,
    // so the holder is identifiable by elimination (the parked thread
    // that is NOT a waiter on the contested set). Volume: <= 24 lines
    // per 30 s on a wedged boot, zero on a healthy one (samgr parks
    // are sub-3s when healthy — the phase-A 3s bar filters them).
    static GRAPH_N: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
    let gn = GRAPH_N.fetch_add(1, Relaxed);
    if gn % 6 == 5 {
        use core::fmt::Write as _;
        let samgr_pid = crate::ipc::binder::binder_sareg_samgr_pid();

        // ---- FUX-MAPS (48th session): dump the samgr process's
        // executable file-backed VMAs ONCE (first graph burst). The
        // wedge anchors (park epc, fault rips, uret candidates) decode
        // against image .so files only with the guest's actual mapping
        // geometry — vma start/end + file offset fingerprints each
        // segment (readelf match offline), no path resolution needed.
        static MAPS_DONE: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        if MAPS_DONE
            .compare_exchange(0, 1, Relaxed, Relaxed)
            .is_ok()
            && samgr_pid != 0
        {
            let mut maps_n = 0usize;
            crate::sched::for_each_task_try(|task| {
                // SAFETY: diagnostic reads under the for_each_task
                // contract.
                let tk = unsafe { &*task };
                // 60th: cap 24 -> 40 — samgr's libffrt/libipc mappings
                // sort AFTER the first 24 file VMAs, and the wedge frame
                // chain (TXN-HANG-BWR ffrt-chain uret addrs) lands in
                // exactly those; without them the frames symbolize as
                // "unmapped".
                if tk.pid() != samgr_pid || maps_n >= 72 {
                    return;
                }
                if let Some(aspace) = tk.address_space() {
                    if let Some(mgr) = aspace.vma_try_read() {
                        use crate::mm::vma::VmaType;
                        for vma in mgr.iter() {
                            if maps_n >= 72 {
                                break;
                            }
                            if !vma.flags().is_executable() {
                                continue;
                            }
                            if !matches!(vma.vma_type(), VmaType::FileBacked) {
                                continue;
                            }
                            // HOTPATH-PRINT: one-shot fuxstuck maps dump.
                            crate::pr_warn!(
                                "FUX-MAPS pid={} {:x}-{:x} off={:#x} fd={}",
                                samgr_pid,
                                vma.start().as_usize(),
                                vma.end().as_usize(),
                                vma.offset(),
                                vma.file_fd()
                            );
                            maps_n += 1;
                        }
                    }
                }
            });
        }
        if samgr_pid != 0 {
            // sc57: name the samgr pid's RUNNABLE threads each burst —
            // an owner stuck in a userspace spin (e.g. the TimerManager
            // spin-yield loops) never parks, so every park-based census
            // is blind to it; a persistent runnable tid is the spin
            // signature.
            {
                use core::fmt::Write as _;
                let mut run_s = alloc::string::String::new();
                let mut nf = 0u32;
                let mut ni = 0u32;
                crate::sched::for_each_task_try(|task| {
                    // SAFETY: diagnostic reads under the for_each_task
                    // contract.
                    let tk = unsafe { &*task };
                    if tk.tgid() as u32 != samgr_pid {
                        return;
                    }
                    let st_bits = tk.state().bits();
                    let run = st_bits == crate::process::task::TaskState::RUNNING
                        || tk.on_cpu()
                        || tk.sched_entity().is_on_rq();
                    if run {
                        let comm = core::str::from_utf8(tk.comm())
                            .unwrap_or("")
                            .trim_end_matches('\0');
                        let _ = write!(run_s, " {}({})", tk.pid(), comm);
                    } else if tk.futex_wait_enqueued.load(Relaxed) {
                        nf += 1;
                    } else if st_bits & 0x1 != 0 {
                        ni += 1;
                    }
                });
                // HOTPATH-PRINT: fuxstuck graph-burst gate.
                crate::pr_warn!(
                    "FUX-RUN samgr_pid={} run:{} futex={} int={}",
                    samgr_pid,
                    run_s,
                    nf,
                    ni
                );
            }
            let mut graph_lines = 0usize;
            for g in groups.iter() {
                if g.tgid != samgr_pid {
                    continue;
                }
                if LINES.fetch_add(1, Relaxed) > 1200 {
                    break;
                }
                let comm_s = core::str::from_utf8(&g.rep_comm)
                    .unwrap_or("")
                    .trim_end_matches('\0');
                let live_s = if g.rep_live_ok {
                    if g.rep_live == (g.rep_fval & 0xffff_ffff) && g.rep_fval != 0 {
                        "live==fval"
                    } else {
                        "LIVE!=fval"
                    }
                } else {
                    "live=?"
                };
                let mut u2v_s = alloc::string::String::new();
                if g.rep_u2v_ok {
                    let _ = write!(
                        u2v_s,
                        "u2v={:#x}+{:#x}{}",
                        g.rep_u2v_start,
                        g.rep_u2v_off,
                        g.rep_u2v_cls as char
                    );
                }
                // HOTPATH-PRINT: fuxstuck graph row — same dfx=fuxstuck
                // gate + 5s burst budget + LINES cap as FUX-STUCK.
                crate::pr_warn!(
                    "FUX-GRAPH tgid={} tid={} ({}) uaddr={:#x} n={} park={}s..{}s anywoken={} fval={:#x} {}={:#x} fto={:#x} A+{:#x} {} uret={}",
                    g.tgid,
                    g.rep_tid,
                    comm_s,
                    g.uaddr,
                    g.n,
                    g.min_age,
                    g.max_age,
                    g.any_woken as u32,
                    g.rep_fval,
                    live_s,
                    g.rep_live,
                    g.rep_fto,
                    g.rep_uvma_off,
                    u2v_s,
                    {
                        let mut s = alloc::string::String::new();
                        for i in 0..g.rep_uret_n {
                            let _ = write!(s, "{:#x} ", g.rep_uret[i]);
                        }
                        s
                    }
                );
                graph_lines += 1;
                // sc57: chain lines for graph rows too (the graph set
                // is the reliable samgr coverage — the 8-row sample is
                // contested by peripheral parks).
                if g.rep_chn_hops.iter().any(|&h| h != 0)
                    || g.rep_chn_prev3.iter().any(|&h| h != 0)
                {
                    let mut hops_s = alloc::string::String::new();
                    for (h, &a) in g.rep_chn_hops.iter().enumerate() {
                        if a == 0 {
                            break;
                        }
                        let _ = write!(
                            hops_s,
                            " {:#x}{}",
                            a,
                            if g.rep_chn_hops_ok & (1 << h) != 0 { "*" } else { "?" }
                        );
                    }
                    let mut prev_s = alloc::string::String::new();
                    for &a in g.rep_chn_prev3.iter() {
                        if a == 0 {
                            break;
                        }
                        let _ = write!(prev_s, " {:#x}", a);
                    }
                    crate::pr_warn!(
                        "FUX-CHAIN tid={} node={:#x} task={:#x} wt={} hops:{} prev:{}",
                        g.rep_tid,
                        g.rep_chn_node,
                        g.rep_chn_task,
                        g.rep_chn_wt,
                        hops_s,
                        prev_s
                    );
                    if g.rep_chn_head != 0 {
                        let mut win_s = alloc::string::String::new();
                        for &wv in g.rep_chn_head_win.iter() {
                            let _ = write!(win_s, "{:#x} ", wv);
                        }
                        crate::pr_warn!(
                            "FUX-CHAIN-HEAD tid={} head={:#x} win:{}",
                            g.rep_tid,
                            g.rep_chn_head,
                            win_s
                        );
                    }
                }
            }
            if graph_lines > 0 {
                crate::pr_warn!(
                    "FUX-GRAPH-SUM samgr_pid={} groups={} (burst {})",
                    samgr_pid,
                    graph_lines,
                    gn
                );
            }
        }
    }
}

/// Allocate a waiter slot
fn alloc_waiter() -> Option<usize> {
    for i in 0..WAITER_POOL_SIZE {
        let mut slot = WAITER_POOL[i].lock_irqsave();
        if slot.is_none() {
            // Reserve INSIDE the critical section: leaving the slot empty
            // until the caller initializes it let two CPUs hand out the
            // same index (review IPC-C2). A placeholder (task == null)
            // makes the slot visibly occupied; chain walkers skip null-task
            // waiters via the key/task checks below.
            *slot = Some(Waiter {
                key: FutexKey::new(0, 0, 0),
                task: core::ptr::null_mut(),
                bitset: 0,
                woken: false,
                park_j: 0,
                bucket: 0,
                next: None,
            });
            FUX_POOL_USED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            return Some(i);
        }
    }
    // sc58: stamp the exhaustion moment — the census reports it; a
    // wedge-time pool-full means FUTEX_WAITs spin in backoff retries,
    // invisible to the bucket-walk census.
    FUX_POOL_FULL_J.store(
        crate::drivers::timer::get_jiffies(),
        core::sync::atomic::Ordering::Relaxed,
    );
    None
}

/// Free a waiter slot
fn free_waiter(index: usize) {
    let mut slot = WAITER_POOL[index].lock_irqsave();
    *slot = None;
    // sc58: occupancy counter (census reads it O(1)).
    FUX_POOL_USED.fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
}

/// Back off one jiffy when the waiter pool is exhausted (see
/// futex_wait_timeout). Sleeps INTERRUPTIBLE with a one-shot timer so a
/// slot frees while we are parked; returns immediately if no task/timer
/// context is available (caller retries anyway).
fn pool_full_backoff() {
    use crate::process::task::{Task, TaskState};

    let current = match crate::sched::current() {
        Some(c) => c as *mut Task,
        None => return,
    };
    let target = crate::drivers::timer::get_jiffies() + 1;
    let pid = unsafe { (*current).pid() };
    // arch-s5: typed one-shot guard — the scope-exit Drop cancels the
    // registration (the old explicit trailing del_timer).
    let Some(wake) = crate::timer::WakeArmed::arm_jiffy(target, pid) else {
        return; // no timer slot: busy retry (rare, bounded by wakers)
    };
    loop {
        if crate::drivers::timer::get_jiffies() >= target {
            break;
        }
        // Mark INTERRUPTIBLE BEFORE the final re-check (state-first
        // lost-wakeup discipline, same as sys_nanosleep).
        // SAFETY: current is the running task's pointer.
        unsafe {
            (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
        }
        if crate::drivers::timer::get_jiffies() >= target {
            // SAFETY: current is the running task's pointer.
            unsafe {
                (*current).set_state(TaskState::new(TaskState::RUNNING));
            }
            // A racing wake may have enqueued us while still executing —
            // take ourselves back off (NEW-C2 discipline).
            // SAFETY: current is the running task's pointer.
            unsafe {
                crate::sched::dequeue_task(&*current);
            }
            break;
        }
        // SAFETY: schedule() operates on the current task.
        unsafe {
            crate::sched::schedule();
        }
    }
    drop(wake);
}

/// Calculate futex hash value
pub(crate) fn futex_hash(key: &FutexKey) -> usize {
    // Hash by the PHYSICAL word identity when resolved (both private and
    // shared keys), else by uaddr. matches() compares the physical word
    // for shared keys — a parent waiting on a MAP_SHARED page at vaddr A
    // and an exec'd child waking from a fresh mmap at vaddr B MUST share
    // a bucket (the LTP tst_checkpoint protocol; the old uaddr-only
    // bucketing put them in different chains and the wake never reached
    // the waiter — waitid08 ETIMEDOUT). Phys-keyed bucketing also keeps
    // the musl interop below inside one bucket: a shared waiter and a
    // private waker on the SAME word resolve the same frame.
    //
    // R31-9 fixed only the shared↔shared direction (mm excluded for
    // shared keys); the 4-thread hang hunt caught the remaining
    // shared-waiter ↔ private-waker split: musl's __thread_list_lock
    // waiters park with priv=0 (shared key), while the kernel's
    // CLONE_CHILD_CLEARTID exit wake (musl pthread_exit relies on it to
    // "unlock" the list lock — it passes &__thread_list_lock as the
    // clone ctid) hashed the private key (uaddr+mm) → a DIFFERENT bucket
    // → the wake scanned an empty chain (woken=0 with three parked
    // waiters) → every later exiter/joiner parked on the never-released
    // lock word forever. Linux solves the same interop with
    // FLAG_IMMUTABLE (the cleartid wake matches both key variants);
    // same-word bucketing gives this kernel the same reach.
    //
    // F12: the raw `uaddr % HASH_SIZE` clustered EVERY thread-stack
    // futex into ONE bucket. musl hands out 1 MB pthread stacks with a
    // fixed stride (0x101000 — stack + guard page), which is a multiple
    // of 64, so the ctid/join futex words of all threads of a process
    // landed in the same bucket: pth_str02's 1000-thread join chain
    // serialised on a single spinlock (all 256 pool slots chained in one
    // bucket, every wake/alloc walking 256 entries under that lock).
    // Fibonacci-multiply the byte offset so any fixed stride spreads
    // across the buckets.
    let id = if key.phys != 0 { key.phys } else { key.uaddr };
    let x = (id >> 2) as u64;
    let mixed = x.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    // Take the high bits (multiplicative hashing) modulo HASH_SIZE.
    let bits = HASH_SIZE.trailing_zeros();
    (mixed >> (64 - bits)) as usize
}

/// Wake up waiters on a futex, keyed on the CURRENT task's mm.
pub fn futex_wake(uaddr: usize, flags: u32, nr_wake: i32, bitset: u32) -> i64 {
    let mm = current_futex_mm_id();
    futex_wake_in_mm(uaddr, mm, flags, nr_wake, bitset)
}

/// Wake up waiters on a futex keyed by an explicit mm identity.
///
/// Used by the exit path (clear_child_tid / robust list) where the waker's
/// CURRENT mm would be wrong or already dropped — the key must be the
/// EXITING task's mm.
///
/// Walks the hash chain for the given futex, waking up to `nr_wake` tasks
/// whose bitset intersects with the requested bitset.  Uses
/// `Task::wake_up()` which properly enqueues the task on the run queue
/// and triggers rescheduling on the target CPU.
pub fn futex_wake_in_mm(uaddr: usize, mm: usize, flags: u32, nr_wake: i32, bitset: u32) -> i64 {
    if bitset == 0 {
        return -EINVAL as i64;
    }

    let key = FutexKey::new_user(uaddr, mm, flags);
    let bucket_idx = futex_hash(&key);
    #[cfg(feature = "dfx-futex-trace")]
    ftx_trace(b"WAKE-IN", uaddr, mm as u64, nr_wake as u64);

    let mut ret = 0i64;
    let mut prev_idx: Option<usize> = None;
    // Collect tasks to wake after releasing the bucket lock, like
    // kernel/futex/waitwake.c wake_futex() + wake_q_add(). Dynamically
    // sized: a fixed 8-entry array silently stranded every waiter past the
    // 8th (pthread_cond_broadcast) — review IPC-C3.
    let mut wake_list: alloc::vec::Vec<*mut Task> = alloc::vec::Vec::new();

    // Hold the hash bucket lock for the entire traversal so no
    // concurrent futex_wait can insert/remove while we walk.
    let mut head = HASH_HEADS[bucket_idx].lock_irqsave();

    let mut current_idx = head.head;
    while let Some(idx) = current_idx {
        if ret >= nr_wake as i64 {
            break;
        }

        // Slot under the bucket guard — proof-checked (D-11 slice 3).
        let mut waiter_slot = WAITER_POOL[idx].lock_nested(&head.proof());
        let should_wake = match *waiter_slot {
            Some(ref waiter) => {
                waiter.key.matches(&key) && (waiter.bitset & bitset) != 0
            }
            None => break,
        };

        if should_wake {
            let woken_task = match *waiter_slot {
                Some(ref waiter) => waiter.task,
                None => break,
            };
            let next_idx = match *waiter_slot {
                Some(ref waiter) => waiter.next,
                None => break,
            };
            // 51st session — cross-mm wake-theft alarm: a PRIVATE-keyed
            // wake consuming a waiter whose key mm differs from the
            // waker's is a key collision (phys-keyed bucketing feeding a
            // shared-keyed waiter to a private waker, or worse). The
            // stolen wake strands the true owner's waiter forever — the
            // frozen-mutex wedge shape. Rate-limited to 32 prints.
            if flags & FLAGS_SHARED == 0 && !woken_task.is_null() {
                let waiter_mm = match *waiter_slot {
                    Some(ref waiter) => waiter.key.mm,
                    None => 0,
                };
                if waiter_mm != 0 && waiter_mm != mm {
                    static THEFT_N: core::sync::atomic::AtomicU32 =
                        core::sync::atomic::AtomicU32::new(0);
                    let tn = THEFT_N
                        .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    if tn < 32 {
                        // SAFETY: read-only comm/pid of a waiter still
                        // linked in this bucket under our lock.
                        unsafe {
                            crate::pr_warn!(
                                "FUTEX-WAKE-THEFT waker mm={:#x} consumed waiter mm={:#x} uaddr={:#x} waiter tid {}",
                                mm,
                                waiter_mm,
                                uaddr,
                                (*woken_task).pid(),
                            );
                        }
                    }
                }
            }

            // Mark woken so futex_wait knows it was explicitly woken.
            if let Some(ref mut w) = *waiter_slot {
                w.woken = true;
            }
            drop(waiter_slot);

            // Unlink from chain (62nd: maintain the FIFO tail — the
            // removed node was the tail iff its next is None).
            if prev_idx.is_none() {
                head.head = next_idx;
                if next_idx.is_none() {
                    head.tail = None;
                }
            } else if let Some(prev) = prev_idx {
                let mut prev_slot = WAITER_POOL[prev].lock_nested(&head.proof());
                if let Some(ref mut pw) = *prev_slot {
                    pw.next = next_idx;
                }
                if next_idx.is_none() {
                    head.tail = prev_idx;
                }
            }

            // Do NOT free the waiter slot here: the sleeping task still
            // inspects it after waking and frees it itself (review IPC-H4).
            // Defer wakeup — collect task pointer, wake after dropping lock.
            if !woken_task.is_null() {
                wake_list.push(woken_task);
            }

            ret += 1;
            current_idx = next_idx;
        } else {
            prev_idx = Some(idx);
            current_idx = match *waiter_slot {
                Some(ref waiter) => waiter.next,
                None => break,
            };
        }
    }

        // R12-2: wake WHILE STILL HOLDING the bucket lock — same reasoning as
        // WaitQueueHead::wake_up (R12-1): the waiter is still linked here, so
        // it cannot have passed its unlink-under-this-lock and exited. Bucket
        // -> GRQ order is safe (no GRQ-held path takes a futex bucket). The
        // old drop-then-wake window was the deferred-wake UAF.
        //
        // C8 wake-ordering invariant (waker side): for every waiter above,
        // (1) the task pointer was extracted from its entry under the slot
        // lock, (2) the `woken` flag was set to true (the slot spinlock's
        // Release store publishes it), and only then (3) Task::wake_up is
        // invoked — in that order. Reordering loses wakeups: the moment
        // wake_up runs, the sleeper is schedulable; a woken sleeper that
        // polls `woken` BEFORE the flag store returns from schedule(),
        // reads false, classifies the wake as spurious, re-checks *uaddr
        // and goes back to sleep — the later `woken = true` is a plain
        // store that schedules no one, and with no second FUTEX_WAKE the
        // waiter sleeps forever despite having been woken once.
        let woken_n = wake_list.len();
    // oh67: for the watched lock words, name WHO the wake consumed —
    // a wake returning ret=1 that consumes a task which never proceeds
    // names a stolen/stale waiter (the frozen-mutex wedge shape).
    let oh67_watch = {
        let a = FUXWATCH_ADDR.load(Relaxed2::Relaxed);
        let b2 = FUXWATCH2_ADDR.load(Relaxed2::Relaxed);
        uaddr as u64 == a || uaddr as u64 == b2
    };
    for task in wake_list {
        if !task.is_null() {
            if oh67_watch {
                static WOKE_N: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                if WOKE_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 64 {
                    // SAFETY: the task is linked in this bucket under our
                    // lock; cannot be freed while we hold it.
                    let tk = unsafe { &*task };
                    crate::pr_warn!(
                        "FUXW-WOKE uaddr={:#x} woken tid={} tgid={} state={:#x}",
                        uaddr,
                        tk.pid(),
                        tk.tgid(),
                        tk.state().bits()
                    );
                }
            }
            // R12-2 wake-under-bucket, now compile-checked: the proof
            // minted by the bucket guard is what the nested GRQ acquire
            // requires (INV-LOCK-5 forward; D-11).
            Task::wake_up_in_futex_bucket(task, &head.proof());
        }
    }
    drop(head);

    // oh67: the lost-notify probe — a FUTEX_WAKE that found NO waiter
    // (ret=0) on a THREAD-STACK word is either a benign timeout-canceled
    // notify or a LOST wake: the notifier's seq bump + wake landed while
    // the waiter was en route to park, or the waiter's node address was
    // stale. musl's cond protocol closes the first window in userspace;
    // any residual ret=0 on a word whose waiter stays parked is the
    // wedge's lost edge. Budget 240/boot, fuxstuck-gated; correlate
    // offline against the stuck-park births (FUX-CTX groups by word).
    if ret == 0
        && (0x7ff0_8000_0000..=0x7ff0_9fff_ffff).contains(&uaddr)
        && crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::FuxStuck)
    {
        static MISS0_N: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        static MISS0_DUP: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let n = MISS0_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 240 {
            let waker = crate::sched::current().map(|t| unsafe {
                ((*t).pid(), (*t).tgid() as u32)
            });
            crate::pr_warn!("FUXW-MISS0 #{} uaddr={:#x} waker={:?}", n, uaddr, waker);
        } else {
            MISS0_DUP.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
    }

    #[cfg(feature = "dfx-futex-trace")]
    ftx_trace(b"WAKE-OUT", uaddr, ret as u64, woken_n as u64);

    // b133 hunt (dfx=syscalltrace): OH's samgr/SA threads park forever in
    // PRIVATE futex_wait inside musl's ld.so malloc locks — 120s+ binder
    // stalls queue the whole display chain behind them. Wake/wait protocol
    // is documented-solid, so the prime suspect is a KEY MISMATCH: a
    // waiter linked under a different (uaddr, mm) than this waker
    // computes. When a wake wanted more waiters than it found, scan every
    // bucket for same-uaddr entries whose key does NOT match, and report
    // both identities. Gated + rate-limited: the scan walks all buckets,
    // and uncontended FUTEX_WAKEs (every pthread_mutex_unlock) must not
    // pay for it.
    if ret < nr_wake as i64
        && crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::FutexTrace)
    {
        static MISS_REPORTS: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let mut waker_pid = 0u32;
        if let Some(t) = crate::sched::current() {
            // SAFETY: sched::current() yields the current task for the
            // duration of this syscall.
            waker_pid = unsafe { (*t).pid() };
        }
        for (bi, b) in HASH_HEADS.iter().enumerate() {
            let mut h = b.lock_irqsave();
            let mut idx = h.head;
            while let Some(i) = idx {
                // SAFETY: the slot index came from a chain link under the
                // bucket lock; slots are freed only under both locks.
                let (task, next, mismatch) = {
                    let slot = WAITER_POOL[i].lock_irqsave();
                    match *slot {
                        Some(ref w) => {
                            let mismatch = w.key.uaddr == uaddr
                                && (!w.key.matches(&key) || (w.bitset & bitset) == 0);
                            (w.task, w.next, mismatch)
                        }
                        None => (core::ptr::null_mut(), None, false),
                    }
                };
                if mismatch {
                    let n = MISS_REPORTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    if n < 32 {
                        let mut wcomm = [0u8; 16];
                        if !task.is_null() {
                            // SAFETY: the task is linked in the bucket; it
                            // cannot be freed while we hold the lock.
                            let c = unsafe { (*task).comm() };
                            wcomm.copy_from_slice(c);
                        }
                        let name = core::str::from_utf8(
                            &wcomm[..wcomm.iter().position(|&c| c == 0).unwrap_or(16)],
                        )
                        .unwrap_or("?");
                        let wmm = {
                            // SAFETY: diagnostic identity read.
                            task_futex_mm_id(task)
                        };
                        crate::pr_warn!(
                            "futex-wake-miss: uaddr={:#x} waker pid {} key(mm={:#x},shared={}) found waiter pid-key mm={:#x} bucket {} waiter-comm {}",
                            uaddr,
                            waker_pid,
                            mm,
                            flags & FLAGS_SHARED != 0,
                            wmm,
                            bi,
                            name
                        );
                    }
                }
                idx = next;
            }
            drop(h);
        }
    }

    ret
}

/// Wait for a futex
///
/// Checks `*uaddr` under the hash bucket lock.  If it still equals `val`,
/// inserts a waiter into the chain, sets state to INTERRUPTIBLE (still under
/// the lock), then drops the lock and schedules.  The "insert + set state
/// under lock" ordering prevents the lost-wakeup race: by the time
/// futex_wake sees the waiter, the task is already in INTERRUPTIBLE state
/// so `Task::wake_up()` (which checks `is_sleeping()`) can succeed.
pub fn futex_wait(uaddr: usize, flags: u32, val: u32, bitset: u32) -> i64 {
    futex_wait_timeout(uaddr, flags, val, bitset, None)
}

/// FUTEX_WAIT with an optional jiffies deadline (see do_futex).
pub fn futex_wait_timeout(uaddr: usize, flags: u32, val: u32, bitset: u32, deadline: Option<u64>) -> i64 {
    if bitset == 0 {
        return -EINVAL as i64;
    }

    let uaddr_ptr = uaddr as *const AtomicU32;
    if uaddr_ptr.is_null() {
        return -EINVAL as i64;
    }

    let current = match crate::sched::current() {
        Some(t) => t,
        None => return -EFAULT as i64,
    };
    // SAFETY: current is the current task's raw pointer from sched::current(),
    // valid for the duration of this syscall.
    let mm = task_futex_mm_id(current);

    let key = FutexKey::new_user(uaddr, mm, flags);
    let bucket_idx = futex_hash(&key);

    // Linux futex_wait semantics: success (0) is returned ONLY when
    // FUTEX_WAKE (or a requeue) explicitly woke us. A spurious wakeup
    // (blocked-signal wake, timer noise) must RE-CHECK the value and go
    // back to sleep — returning 0 here let LTP's checkpoint handshake
    // (sighold02: child holds all signals, parent kills 60 of them, then
    // FUTEX_WAKEs) complete the wait early: the parent's wake found no
    // waiter and TBROK'd with ETIMEDOUT.
    loop {
        // Lock the hash bucket.  All subsequent operations (value check,
        // waiter insertion, state change) happen under this lock.
        let mut head = HASH_HEADS[bucket_idx].lock_irqsave();

        // Re-check value under lock (prevents lost wakeup).
        // SAFETY: get_user goes through the exception-table copy path; an
        // unmapped user address yields EFAULT instead of a kernel page fault.
        let uval = match unsafe {
            crate::arch::uaccess::get_user(uaddr_ptr as *const u32)
        } {
            Some(v) => v,
            None => return -EFAULT as i64,
        };
        #[cfg(feature = "dfx-futex-trace")]
        ftx_trace(b"WAIT-VAL", uaddr, uval as u64, val as u64);
        if uval != val {
            return -EAGAIN as i64;
        }

        // Deadline already passed while we were re-checking?
        // 63rd: deadlines are absolute monotonic ns now (was: jiffies).
        if let Some(dl) = deadline {
            if crate::drivers::timer::monotonic_ns_since_boot() >= dl {
                return -ETIMEDOUT as i64;
            }
        }

        // Allocate waiter slot.
        //
        // F12: a full pool must NEVER fail the wait with ENOMEM. Linux
        // parks futex waiters on a futex_q living on the CALLER'S KERNEL
        // STACK (futex_wait_queue), so a futex wait cannot fail for
        // capacity reasons, and userspace relies on that: musl's join /
        // __timedwait loops only exit on ETIMEDOUT/EINVAL and blindly
        // re-issue FUTEX_WAIT on any other error. With ENOMEM here,
        // pth_str02's 1000-thread join chain live-locked — every joiner
        // spinning lock-acquire → full-pool scan → ENOMEM, starving the
        // exit-path wakes that would have freed slots. Instead: drop the
        // bucket lock, back off one jiffy, and retry the whole sequence
        // (value re-check included). Slots free as wakes progress; the
        // exit path (ctid wake) never needs a slot, so forward progress
        // is guaranteed.
        let waiter_idx = match alloc_waiter() {
            Some(idx) => idx,
            None => {
                drop(head);
                pool_full_backoff();
                continue;
            }
        };

        // Initialize and insert waiter into hash chain (fill the placeholder
        // reserved by alloc_waiter in place). 62nd: TAIL append — FIFO
        // waiter order, reference plist parity (see FutexChain). The
        // OLDEST waiter must receive the first wake of every burst; the
        // head-insert (LIFO) starved the long-parked M2 holder behind
        // the samgr workers' transient re-parkers (ohw27/28).
        {
            let mut slot = WAITER_POOL[waiter_idx].lock_irqsave();
            if let Some(ref mut w) = *slot {
                w.key = key;
                w.task = current;
                w.bitset = bitset;
                w.woken = false;
                w.park_j = crate::drivers::timer::get_jiffies();
                w.bucket = bucket_idx;
                w.next = None;
            }
        }

        // Append at the chain tail.
        match head.tail {
            Some(t) => {
                let mut tslot = WAITER_POOL[t].lock_nested(&head.proof());
                if let Some(ref mut tw) = *tslot {
                    tw.next = Some(waiter_idx);
                }
                head.tail = Some(waiter_idx);
            }
            None => {
                head.head = Some(waiter_idx);
                head.tail = Some(waiter_idx);
            }
        }

        // Residency flag (see Task::futex_wait_enqueued): linked into the
        // chain here, cleared by every unlink/free path — do_exit's
        // futex_cleanup gates its 512-bucket scan on it. We still hold the
        // bucket lock; the flag is only read by this task's own exit path,
        // so a plain store suffices.
        unsafe {
            (*current)
                .futex_wait_enqueued
                .store(true, core::sync::atomic::Ordering::Release);
        }

        // oh67: name WHO parks on the watched lock words (the frozen-mutex
        // wedge: a waiter parked on M2.l that tid343-class unlocks keep
        // consuming without the web progressing).
        {
            let a = FUXWATCH_ADDR.load(Relaxed2::Relaxed);
            let b2 = FUXWATCH2_ADDR.load(Relaxed2::Relaxed);
            if uaddr as u64 == a || uaddr as u64 == b2 {
                static PARK_N: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                if PARK_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 64 {
                    crate::pr_warn!(
                        "FUXW-PARK uaddr={:#x} tid={} tgid={} val={:#x}",
                        uaddr,
                        current.pid(),
                        current.tgid(),
                        val
                    );
                }
            }
        }

        // C8 wake-ordering invariant (sleeper side): in order — (1) our
        // waiter entry (task pointer, the flag the waker polls) is linked
        // into the bucket chain, (2) the task state is stored as
        // INTERRUPTIBLE with Release, both strictly BEFORE (3) the bucket
        // lock is dropped and schedule() runs. The waker (futex_wake /
        // futex_requeue) takes the same lock, so it can never observe the
        // entry without also observing the sleeping state — Task::wake_up
        // drops every wake of a non-sleeping task. Reordering loses
        // wakeups: linked-but-RUNNING lets the waker consume the wake
        // (entry marked woken, task never enqueued), and our later state
        // store + schedule() then sleep forever; symmetrically, the
        // post-schedule `woken` poll below must come AFTER the waker's
        // flag store, which the waker guarantees by setting `woken`
        // before it invokes Task::wake_up (see futex_wake_in_mm).
        //
        // Set task state to INTERRUPTIBLE while still holding the hash lock.
        // This guarantees that any futex_wake that sees the waiter in the chain
        // will also see the task in INTERRUPTIBLE state, preventing the
        // lost-wakeup race.
        // SAFETY: current is the current task, valid for the duration of this
        // function.  We hold the hash bucket lock so futex_wake will see the
        // state transition before checking is_sleeping().
        unsafe {
            (*current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
        }

        // Release the hash bucket lock.  The Release semantics ensure that the
        // waiter entry (chain + INTERRUPTIBLE state) is visible to other
        // CPUs before they can observe the lock is free.
        drop(head);

        // Schedule — yields the CPU.  The task will be re-enqueued by
        // Task::wake_up() when futex_wake (or a signal) wakes it.  Arm a
        // wakeup timer when a deadline is set: nothing else would wake a
        // futex that is never signaled (pthread_cond_timedwait would hang).
        crate::arch::cpu::restore_irq(true);
        // 63rd: HIGH-RESOLUTION wake at the precise ns deadline — the
        // jiffy-grid arm fired every sub-10 ms FFRT timed wait at the
        // next 10 ms tick (reference hrtimer parity; same discipline as
        // nanosleep/sigtimedwait). The jiffy value is wheel bookkeeping
        // only (jiffies_covering_deadline ceils so the wheel entry never
        // precedes the hres expiry); drop(wake) below deletes the timer
        // after schedule() returns.
        let wake = deadline.map(|dl| {
            let id = crate::timer::add_timer_wakeup_hres(
                crate::syscall::misc::jiffies_covering_deadline(dl),
                crate::syscall::misc::deadline_ns_to_hres_ticks(dl),
                crate::sched::get_current_pid(),
            );
            crate::drivers::timer::rearm_for_hres();
            id
        });
        // R32 (NEW-3): if a deadline was requested but the timer pool is
        // exhausted (arm → 0), schedule() below would sleep
        // FOREVER — nothing else wakes an un-signaled futex, so the timed
        // wait degenerated into an untimed hang. Unlink and fail instead.
        if deadline.is_some() && wake == Some(0) {
            remove_waiter(bucket_idx, waiter_idx);
            return -ENOMEM as i64;
        }
        crate::sched::schedule();
        if let Some(id) = wake {
            if id != 0 {
                crate::timer::del_timer(id);
            }
        }
        #[cfg(feature = "dfx-futex-trace")]
        ftx_trace(b"WAIT-RET", uaddr, val as u64, 0);

        // The waiter's bucket may have changed while we slept (FUTEX_REQUEUE
        // moved us to uaddr2's bucket). Re-read it from the slot so the
        // removal below hits the right chain.
        let live_bucket = {
            let slot = WAITER_POOL[waiter_idx].lock_irqsave();
            slot.as_ref().map(|w| w.bucket).unwrap_or(bucket_idx)
        };

        // Check for signal interruption (EINTR). Ownership guard: only act on
        // the slot if it still belongs to us (task pointer matches).
        {
            let slot = WAITER_POOL[waiter_idx].lock_irqsave();
            let mine = slot.as_ref().map(|w| w.task == current).unwrap_or(false);
            drop(slot);
            if !mine {
                // Slot was recycled underneath us (should not happen now that
                // the waker never frees slots, but stay defensive).
                return 0;
            }
            if crate::signal::signal_pending() {
                let woken = {
                    let slot = WAITER_POOL[waiter_idx].lock_irqsave();
                    slot.as_ref().map(|w| w.woken).unwrap_or(false)
                };
                if !woken {
                    remove_waiter(live_bucket, waiter_idx);
                    return -crate::syscall::errno::EINTR as i64;
                }
            }
        }

        // After waking up, check if we were explicitly woken. The waiter owns
        // its slot: unlink paths that did not wake us leave it in the chain
        // (remove), the waker unlinked it already (just free the slot).
        let mut was_spurious = false;
        {
            let mut slot = WAITER_POOL[waiter_idx].lock_irqsave();
            let mine = slot.as_ref().map(|w| w.task == current).unwrap_or(false);
            if mine {
                let was_woken = slot.as_ref().map(|w| w.woken).unwrap_or(false);
                if !was_woken {
                    // Not explicitly woken (spurious wakeup or the timeout
                    // timer): still in the chain.
                    drop(slot);
                    remove_waiter(live_bucket, waiter_idx);
                    // Timeout semantics (review 2R.9): if we were not woken and
                    // the deadline has passed, this is a genuine ETIMEDOUT —
                    // returning success here broke every timed waiter.
                    // 63rd: absolute monotonic-ns deadlines.
                    if let Some(dl) = deadline {
                        if crate::drivers::timer::monotonic_ns_since_boot() >= dl {
                            return -ETIMEDOUT as i64;
                        }
                    }
                    // Spurious: loop — re-check the value and sleep again.
                    was_spurious = true;
                } else {
                    // Woken: the waker unlinked us from the chain and left the
                    // slot for us to free.
                    *slot = None;
                    // sc58: occupancy counter (paired with alloc_waiter).
                    FUX_POOL_USED.fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
                    // Residency flag (see Task::futex_wait_enqueued): the
                    // waker's unlink left the flag set for us to clear here.
                    unsafe {
                        (*current)
                            .futex_wait_enqueued
                            .store(false, core::sync::atomic::Ordering::Release);
                    }
                }
            }
        }
        if !was_spurious {
            return 0;
        }
        // else: continue the loop (Linux futex_wait re-sleeps).
    }
}

/// Remove waiter from hash chain.
///
/// Reference parity (kernel/futex/core.c futex_unqueue): the caller read
/// `w.bucket` WITHOUT any bucket lock, and FUTEX_REQUEUE can relink a
/// sleeping waiter into a different bucket's chain between that read and
/// the lock acquisition here. Re-read the waiter's CURRENT bucket under
/// the acquired bucket lock and retry with the fresh one. The invariant
/// exploited: `w.bucket` names the chain that contains the node, the
/// requeue updates both under BOTH bucket locks, and once we hold the
/// lock named by a fresh read, no requeue can move the node before we
/// unlink it (it would need the lock we hold).
///
/// Without this, a remove that raced a requeue walked the STALE chain,
/// found nothing, and silently returned — leaving the waiter linked in
/// the NEW chain while its task returned to userspace. The stale entry
/// then ate later FUTEX_WAKEs on the requeue target (the missed-wakeup
/// shape the FFRT mutex-word park census flagged): the wake's budget was
/// consumed unlinking a ghost whose task was long gone.
fn remove_waiter(bucket_idx: usize, target_idx: usize) {
    let mut b = bucket_idx;
    loop {
        let mut head = HASH_HEADS[b].lock_irqsave();

        // Revalidate under the bucket lock (futex_unqueue's lock_ptr
        // recheck-retry). Slot lock serializes against requeue's relink
        // writes to the same fields.
        let live = {
            let slot = WAITER_POOL[target_idx].lock_irqsave();
            match *slot {
                Some(ref w) if !w.task.is_null() => w.bucket,
                // Slot already freed or recycled underneath us: nothing
                // to remove (reference: q->lock_ptr == NULL case).
                _ => return,
            }
        };
        if live != b {
            drop(head);
            b = live;
            continue;
        }

        if head.head == Some(target_idx) {
            let next = {
                let slot = WAITER_POOL[target_idx].lock_irqsave();
                let n = slot.as_ref().and_then(|w| w.next);
                // Residency flag: this unlink takes the waiter out of every
                // chain (see Task::futex_wait_enqueued).
                if let Some(w) = slot.as_ref() {
                    if !w.task.is_null() {
                        unsafe {
                            (*w.task).futex_wait_enqueued
                                .store(false, core::sync::atomic::Ordering::Release);
                        }
                    }
                }
                n
            };
            head.head = next;
            if next.is_none() {
                // the removed head was also the tail
                head.tail = None;
            }
            free_waiter(target_idx);
            return;
        }

        let mut current_idx = head.head;
        let mut found = false;
        while let Some(idx) = current_idx {
            let next = {
                let slot = WAITER_POOL[idx].lock_irqsave();
                slot.as_ref().and_then(|w| w.next)
            };

            if next == Some(target_idx) {
                let target_next = {
                    let target_slot = WAITER_POOL[target_idx].lock_irqsave();
                    let n = target_slot.as_ref().and_then(|w| w.next);
                    if let Some(w) = target_slot.as_ref() {
                        if !w.task.is_null() {
                            unsafe {
                                (*w.task).futex_wait_enqueued
                                    .store(false, core::sync::atomic::Ordering::Release);
                            }
                        }
                    }
                    n
                };
                {
                    let mut slot = WAITER_POOL[idx].lock_irqsave();
                    if let Some(ref mut w) = *slot {
                        w.next = target_next;
                    }
                }
                if target_next.is_none() {
                    // the removed node was the tail: prev becomes the tail
                    head.tail = Some(idx);
                }
                free_waiter(target_idx);
                found = true;
                break;
            }

            current_idx = next;
        }
        if !found {
            // Unreachable while the invariants hold (we held the bucket
            // the waiter itself says it is linked into): surface it
            // rather than silently leaking the slot, but do not spin.
            crate::pr_warn!(
                "futex-remove-miss: slot {} bucket {} not in chain",
                target_idx,
                b
            );
        }
        return;
    }
}

/// Clean up all futex waiters for a given task.
///
/// Called from `do_exit` so that no dangling waiter entries remain in
/// the hash chains after the task is freed.  Wakes the task (so it can
/// continue exiting) and frees all its waiter slots.
pub fn futex_cleanup(task: *mut Task) {
    if task.is_null() {
        return;
    }

    // Fast path (perf-mm lane, 2026-10-31): the residency flag is clear
    // unless this task died with a waiter entry still linked (kill raced
    // the sleeper-side unlink). The common exit — never parked on a
    // futex, or the wait returned and unlinked itself — skips the whole
    // 512-bucket scan (one lock_irqsave pair per bucket per exit, the
    // dominant cost of the exit "rest" block: measured ~50-80us of the
    // ~90us block under TCG).
    // SAFETY: do_exit calls this in the task's own context; no other CPU
    // stores the flag for this task concurrently.
    if !unsafe { (*task).futex_wait_enqueued.load(core::sync::atomic::Ordering::Acquire) } {
        return;
    }

    for bucket_idx in 0..HASH_SIZE {
        let mut head = HASH_HEADS[bucket_idx].lock_irqsave();
        let mut prev_idx: Option<usize> = None;
        let mut current_idx = head.head;

        while let Some(idx) = current_idx {
            // Match on the task POINTER (unique per waiter). The old pid
            // match broke once threads shared a tgid side of the key.
            let remove = {
                let slot = WAITER_POOL[idx].lock_irqsave();
                slot.as_ref().map_or(false, |w| w.task == task)
            };

            if remove {
                // Unlink from chain.
                let next = {
                    let slot = WAITER_POOL[idx].lock_irqsave();
                    slot.as_ref().and_then(|w| w.next)
                };
                if prev_idx.is_none() {
                    head.head = next;
                    if next.is_none() {
                        head.tail = None;
                    }
                } else if let Some(prev) = prev_idx {
                    let mut prev_slot = WAITER_POOL[prev].lock_irqsave();
                    if let Some(ref mut pw) = *prev_slot {
                        pw.next = next;
                    }
                    if next.is_none() {
                        head.tail = prev_idx;
                    }
                }
                free_waiter(idx);
                current_idx = next;
            } else {
                prev_idx = Some(idx);
                current_idx = {
                    let slot = WAITER_POOL[idx].lock_irqsave();
                    slot.as_ref().and_then(|w| w.next)
                };
            }
        }
        // Release bucket lock here (end of iteration — `head` dropped on next loop or at end).
        drop(head);
    }

    // Wake the task so it can continue the exit path — done outside all bucket locks.
    // C8: no `woken` flag is needed here (unlike futex_wake) because the
    // target never re-sleeps: do_exit calls this after the task's last
    // schedule(), so there is no sleeper-side poll that a reordered wake
    // could strand.
    // Residency flag: the scan above removed every chained entry of this
    // task — the flag is now false for good (this is the task's own exit
    // context; no new wait can start).
    // SAFETY: same context argument as the load above.
    unsafe {
        (*task).futex_wait_enqueued.store(false, core::sync::atomic::Ordering::Release);
    }
    Task::wake_up(task);
}

/// ENOMEM
const ENOMEM: i32 = 12;

/// FUTEX_WAIT_BITSET implementation
pub fn futex_wait_bitset(uaddr: usize, flags: u32, val: u32, _timeout: u64, bitset: u32, deadline: Option<u64>) -> i64 {
    futex_wait_timeout(uaddr, flags, val, bitset, deadline)
}

/// Parse the futex ABI `struct timespec *timeout` (raw user pointer) into a
/// jiffies deadline.
///
/// - `Ok(None)`: NULL timeout — wait forever.
/// - `Ok(Some(dl))`: parsed deadline.
/// - `Err(EFAULT)`: non-NULL but unreadable / invalid timespec. Linux
///   returns EFAULT instead of silently waiting forever.
///
/// `absolute`: FUTEX_WAIT_BITSET (and FUTEX_WAIT|FUTEX_CLOCK_REALTIME) pass
/// an absolute timespec; CLOCK_REALTIME here counts from boot (CLINT cycles
/// / TIMER_CLOCK_FREQ_HZ) and so does jiffies, so the conversion needs no
/// offset. Plain FUTEX_WAIT passes a relative duration (round 6 MED: was
/// always relative, so every pthread_cond_timedwait fired instantly or
/// never).
/// Parse a futex timeout into an ABSOLUTE monotonic-ns deadline.
///
/// 63rd session (strand-wall/wedge hunt): the old conversion quantized
/// every timed futex wait onto the jiffy grid (HZ=100 → 10 ms) —
/// `(nsec*HZ)/1e9` TRUNCATED and a `.max(1)` floor made every sub-10 ms
/// FFRT timed wait (SerialQueue::Pull's cond_.wait_for(delay),
/// musl cv timeouts) fire at exactly the next 10 ms tick, and waits of
/// 10-20 ms could fire a full grid step EARLY. The reference arms
/// futex timeouts on hrtimers (ns resolution, never early); musl in
/// the OH image parks cv waiters via plain relative FUTEX_WAIT (op
 /// 0x80 in every FUX-STUCK row), so this path carries EVERY timed
/// pthread synchronization edge. Timed waits now convert exactly:
/// relative = now_ns + spec_ns (ns-exact, never early); WAIT_BITSET
/// keeps Linux's CLOCK_MONOTONIC-absolute default (the value is
/// boot-relative ns — same timebase as monotonic_ns_since_boot), and
/// FUTEX_CLOCK_REALTIME absolute values are rebased through the wall
/// epoch offset (the old jiffy arithmetic compared 1970-epoch seconds
/// against boot jiffies — a 2026-epoch deadline read as ~365 days of
/// monotonic wait and never expired; the ohw30 census caught exactly
/// that shape, fto=31535994s).
pub fn futex_parse_timeout_ns(
    timeout_ptr: u64,
    absolute: bool,
    clock_realtime: bool,
) -> Result<Option<u64>, i32> {
    if timeout_ptr == 0 {
        return Ok(None);
    }
    if !crate::arch::uaccess::access_ok(timeout_ptr as usize, 16) {
        return Err(EFAULT);
    }
    let mut buf = [0u8; 16];
    // SAFETY: access_ok-validated user pointer; exception-table copy.
    let uncopied = unsafe {
        crate::arch::uaccess::copy_from_user(
            buf.as_mut_ptr(), timeout_ptr as *const u8, 16,
        )
    };
    if uncopied > 0 {
        return Err(EFAULT);
    }
    let sec = i64::from_le_bytes(buf[0..8].try_into().unwrap());
    let nsec = i64::from_le_bytes(buf[8..16].try_into().unwrap());
    if sec < 0 || nsec < 0 || nsec >= 1_000_000_000 {
        return Err(EINVAL);
    }
    let spec_ns = (sec as u64)
        .saturating_mul(1_000_000_000)
        .saturating_add(nsec as u64);
    if !absolute {
        // Relative (FUTEX_WAIT, futex(2) WAIT): Linux futex_init_timeout
        // adds ktime_get() regardless of FUTEX_CLOCK_REALTIME.
        return Ok(Some(
            crate::drivers::timer::monotonic_ns_since_boot().saturating_add(spec_ns),
        ));
    }
    if clock_realtime {
        // WAIT_BITSET + FUTEX_CLOCK_REALTIME: absolute against the
        // realtime clock — rebase onto the monotonic timebase the wake
        // timers use. A deadline already in the past saturates to 0 and
        // expires on the next check (Linux returns ETIMEDOUT promptly).
        let wall = crate::syscall::time::wall_epoch_offset_ns() as u128;
        let spec = spec_ns as u128;
        Ok(Some(spec.saturating_sub(wall) as u64))
    } else {
        // WAIT_BITSET default: absolute CLOCK_MONOTONIC — the timespec
        // is already boot-relative ns.
        Ok(Some(spec_ns))
    }
}

/// FUTEX_WAKE_BITSET implementation
pub fn futex_wake_bitset(uaddr: usize, flags: u32, nr_wake: i32, bitset: u32) -> i64 {
    futex_wake(uaddr, flags, nr_wake, bitset)
}

/// Convert FUTEX opcode to internal flags
pub fn futex_to_flags(op: u32) -> u32 {
    let mut flags = 0u32;

    if (op & FUTEX_PRIVATE_FLAG as u32) == 0 {
        flags |= FLAGS_SHARED;
    }

    if (op & FUTEX_CLOCK_REALTIME as u32) != 0 {
        flags |= FLAGS_CLOCKRT;
    }

    flags
}

/// FUTEX_REQUEUE / FUTEX_CMP_REQUEUE implementation.
///
/// Reference shape (kernel/futex/requeue.c futex_requeue, non-PI path):
/// - the cmpval check reads `*uaddr` UNDER the bucket lock(s), so the
///   "generation unchanged" verdict is atomic with the chain migration
///   (EAGAIN is decided while holding the same locks the waiters queue
///   under — our futex_wait also re-checks the value under its bucket
///   lock, which is what makes the pair airtight);
/// - the scan wakes up to `nr_wake` matched waiters (unlink + flag under
///   the locks) and requeues up to `nr_requeue` more by unlinking them
///   from uaddr's chain and linking them into uaddr2's chain, all under
///   BOTH bucket locks — a requeued waiter is never observable in limbo;
/// - requeued waiters are appended at the TAIL of the destination chain
///   (reference requeue_futex's plist_add: FIFO within the priority
///   class — a requeued waiter must not overtake waiters already queued
///   on uaddr2);
/// - woken tasks are woken while the bucket guards are still held
///   (R12-2, same invariant as futex_wake_in_mm: the deferred
///   wake-after-unlock used to leave a UAF window where a signal-roused
///   waiter returned, exited and was swept before Task::wake_up ran).
///
/// Returns the total number of waiters woken + requeued, or a negative errno.
pub fn futex_requeue(
    uaddr: usize,
    flags: u32,
    nr_wake: i32,
    nr_requeue: i32,
    uaddr2: usize,
    cmpval: u32,
    is_cmp: bool,
) -> i64 {
    let mm = current_futex_mm_id();

    let key1 = FutexKey::new_user(uaddr, mm, flags);

    // Degenerate shapes (dispatch already rejects uaddr2 == 0, but this
    // fn is reachable from other callers): same key or nothing to requeue
    // still runs the cmpval check under the bucket lock first — the
    // reference does not shortcut to futex_wake, so CMP_REQUEUE with a
    // stale value must fail with EAGAIN instead of waking.
    if uaddr2 == 0 || uaddr2 == uaddr || nr_requeue <= 0 {
        let mut head1 = HASH_HEADS[futex_hash(&key1)].lock_irqsave();
        if is_cmp {
            // SAFETY: exception-table protected read; unmapped → EFAULT.
            match unsafe { crate::arch::uaccess::get_user(uaddr as *const u32) } {
                Some(uval) if uval == cmpval => {}
                Some(_) => return -EAGAIN as i64,
                None => return -EFAULT as i64,
            }
        }
        drop(head1);
        return futex_wake(uaddr, flags, nr_wake, FUTEX_BITSET_MATCH_ANY);
    }

    let key2 = FutexKey::new_user(uaddr2, mm, flags);
    let bucket1 = futex_hash(&key1);
    let bucket2 = futex_hash(&key2);

    let mut ret = 0i64;
    let mut woken = 0i32;
    let mut requeued = 0i32;

    // Collect tasks to wake. Dynamically sized: fixed 8/32-entry arrays
    // silently stranded waiters past their capacity (pthread_cond_broadcast)
    // — review IPC-C3/M7.
    let mut wake_list: alloc::vec::Vec<*mut Task> = alloc::vec::Vec::new();

    // Lock both buckets to prevent requeued entries from being in limbo.
    // Use index ordering (lower first) to avoid ABBA deadlock — same
    // ordering as the reference's double_lock_hb on bucket address.
    if bucket1 == bucket2 {
        // Same bucket — lock once (reference double_lock_hb takes hb1 only
        // when hb1 == hb2).
        let mut head1 = HASH_HEADS[bucket1].lock_irqsave();

        // CMP_REQUEUE: verify *uaddr == cmpval UNDER the bucket lock
        // (reference futex_get_value_locked + EAGAIN at out_unlock).
        if is_cmp {
            // SAFETY: exception-table protected read; unmapped → EFAULT.
            match unsafe { crate::arch::uaccess::get_user(uaddr as *const u32) } {
                Some(uval) if uval == cmpval => {}
                Some(_) => return -EAGAIN as i64,
                None => return -EFAULT as i64,
            }
        }

        let mut prev: Option<usize> = None;
        let mut cur = head1.head;

        while let Some(idx) = cur {
            let (matches, next) = {
                let slot = WAITER_POOL[idx].lock_irqsave();
                match *slot {
                    Some(ref w) => (w.key.matches(&key1), w.next),
                    None => break,
                }
            };

            if !matches {
                prev = Some(idx);
                cur = next;
                continue;
            }

            if woken < nr_wake {
                let task = {
                    let slot = WAITER_POOL[idx].lock_irqsave();
                    slot.as_ref().map(|w| w.task).unwrap_or(core::ptr::null_mut())
                };
                if prev.is_none() {
                    head1.head = next;
                    if next.is_none() {
                        head1.tail = None;
                    }
                } else if let Some(p) = prev {
                    let mut ps = WAITER_POOL[p].lock_irqsave();
                    if let Some(ref mut pw) = *ps { pw.next = next; }
                    if next.is_none() {
                        head1.tail = prev;
                    }
                }
                {
                    let mut slot = WAITER_POOL[idx].lock_irqsave();
                    if let Some(ref mut w) = *slot { w.woken = true; }
                }
                // The woken waiter frees its own slot (see futex_wait).

                if !task.is_null() {
                    wake_list.push(task);
                }
                woken += 1;
                ret += 1;
                cur = next;
            } else if requeued < nr_requeue {
                // Same bucket: the reference's requeue_futex only updates
                // the key when hb1 == hb2 — the node keeps its position.
                {
                    let mut slot = WAITER_POOL[idx].lock_irqsave();
                    if let Some(ref mut w) = *slot {
                        w.key = key2;
                        w.bucket = bucket2;
                    }
                }
                requeued += 1;
                ret += 1;
                prev = Some(idx);
                cur = next;
            } else {
                break;
            }
        }

        // R12-2: wake while the bucket guard is still held — the
        // flag-then-wake order (C8) is unchanged, but the Task deref is
        // bounded by the guard exactly like futex_wake_in_mm. A
        // signal-roused waiter that returned early cannot complete
        // do_exit past futex_cleanup's acquire of this bucket before the
        // wake below finishes; the pid revalidation stays as defense in
        // depth for the residency-fastpath residue.
        for task in &wake_list {
            if !task.is_null() {
                let pid = unsafe { (**task).pid() };
                let fresh = crate::process::pid_hash::pid_hash_lookup(pid);
                if fresh == *task {
                    Task::wake_up_in_futex_bucket(*task, &head1.proof());
                }
            }
        }
        drop(head1);
    } else {
        // Different buckets — lock both with deadlock-avoidance ordering.
        let (lo, hi) = if bucket1 < bucket2 {
            (bucket1, bucket2)
        } else {
            (bucket2, bucket1)
        };

        let mut guard_lo = HASH_HEADS[lo].lock_irqsave();
        let mut guard_hi = HASH_HEADS[hi].lock_irqsave();

        // Get references to the actual heads we need.
        let (head1_ref, head2_ref) = if bucket1 < bucket2 {
            (&mut *guard_lo, &mut *guard_hi)
        } else {
            (&mut *guard_hi, &mut *guard_lo)
        };

        // CMP_REQUEUE: verify *uaddr == cmpval UNDER both bucket locks
        // (reference futex_get_value_locked inside the double_lock_hb
        // region). Our get_user goes through the exception table, so the
        // in-lock read cannot sleep-block on an unmapped word: it returns
        // None → EFAULT, where the reference drops the locks, faults the
        // page in with get_user, and retries — private futex words are
        // always resident here, so the observable contract matches.
        if is_cmp {
            // SAFETY: exception-table protected read; unmapped → EFAULT.
            match unsafe { crate::arch::uaccess::get_user(uaddr as *const u32) } {
                Some(uval) if uval == cmpval => {}
                Some(_) => return -EAGAIN as i64,
                None => return -EFAULT as i64,
            }
        }

        let mut prev: Option<usize> = None;
        let mut cur = head1_ref.head;

        while let Some(idx) = cur {
            let (matches, next) = {
                let slot = WAITER_POOL[idx].lock_irqsave();
                match *slot {
                    Some(ref w) => (w.key.matches(&key1), w.next),
                    None => break,
                }
            };

            if !matches {
                prev = Some(idx);
                cur = next;
                continue;
            }

            if woken < nr_wake {
                let task = {
                    let slot = WAITER_POOL[idx].lock_irqsave();
                    slot.as_ref().map(|w| w.task).unwrap_or(core::ptr::null_mut())
                };
                if prev.is_none() {
                    head1_ref.head = next;
                    if next.is_none() {
                        head1_ref.tail = None;
                    }
                } else if let Some(p) = prev {
                    let mut ps = WAITER_POOL[p].lock_irqsave();
                    if let Some(ref mut pw) = *ps { pw.next = next; }
                    if next.is_none() {
                        head1_ref.tail = prev;
                    }
                }
                {
                    let mut slot = WAITER_POOL[idx].lock_irqsave();
                    if let Some(ref mut w) = *slot { w.woken = true; }
                }
                // The woken waiter frees its own slot (see futex_wait).

                if !task.is_null() {
                    wake_list.push(task);
                }
                woken += 1;
                ret += 1;
                cur = next;
            } else if requeued < nr_requeue {
                // Unlink from source chain.
                if prev.is_none() {
                    head1_ref.head = next;
                    if next.is_none() {
                        head1_ref.tail = None;
                    }
                } else if let Some(p) = prev {
                    let mut ps = WAITER_POOL[p].lock_irqsave();
                    if let Some(ref mut pw) = *ps { pw.next = next; }
                    if next.is_none() {
                        head1_ref.tail = prev;
                    }
                }
                // Relink at the TAIL of the destination chain and update
                // the key + bucket bookkeeping under the same slot lock,
                // so a concurrently-signalled sleeper reads a consistent
                // (bucket, key, next) triple from its own slot. Tail
                // append, not head push: reference requeue_futex inserts
                // with plist_add (FIFO within a priority class), so this
                // waiter must not overtake waiters already queued on
                // uaddr2 — head insertion inverted that order, letting a
                // just-requeued condvar waiter steal the mutex handoff
                // wake from a longer-blocked mutex waiter.
                {
                    let mut slot = WAITER_POOL[idx].lock_irqsave();
                    if let Some(ref mut w) = *slot {
                        w.key = key2;
                        w.bucket = bucket2;
                        w.next = None;
                    }
                }
                // 62nd: the bucket now carries its tail — O(1) append
                // (the old version walked the whole destination chain
                // under both bucket locks to find it).
                match head2_ref.tail {
                    None => {
                        head2_ref.head = Some(idx);
                        head2_ref.tail = Some(idx);
                    }
                    Some(t) => {
                        // Plain slot lock (not nested): the requeue holds
                        // BOTH bucket guards here — the D-11 same-class
                        // double-slot hold this path already uses for its
                        // relink writes.
                        let mut tslot = WAITER_POOL[t].lock_irqsave();
                        if let Some(ref mut tw) = *tslot {
                            tw.next = Some(idx);
                        }
                        head2_ref.tail = Some(idx);
                    }
                }

                requeued += 1;
                ret += 1;
                cur = next;
            } else {
                break;
            }
        }

        // R12-2: wake while BOTH bucket guards are still held (see the
        // same-bucket branch above; the proof of either guard satisfies
        // the nested GRQ acquire — INV-LOCK-5 class crossing).
        for task in &wake_list {
            if !task.is_null() {
                let pid = unsafe { (**task).pid() };
                let fresh = crate::process::pid_hash::pid_hash_lookup(pid);
                if fresh == *task {
                    Task::wake_up_in_futex_bucket(*task, &guard_lo.proof());
                }
            }
        }
        drop(guard_lo);
        drop(guard_hi);
    }

    ret
}

/// FUTEX_OP_OPARG_SHIFT (include/uapi/linux/futex.h): bit 3 of the op
/// nibble makes oparg a SHIFT AMOUNT (oparg = 1 << oparg) instead of an
/// immediate.
pub const FUTEX_OP_OPARG_SHIFT: u32 = 1 << 3;

/// The decoded FUTEX_WAKE_OP operand encoding.
///
/// Reference contract (kernel/futex/waitwake.c futex_atomic_op_inuser):
/// - op nibble is 3 bits + the OPARG_SHIFT bit: `(encoded & 0x70000000) >> 28`
///   selects the operation, bit 3 (`0x80000000`) turns oparg into a shift
///   amount;
/// - oparg and cmparg are 12-bit SIGNED fields (sign_extend32(..., 11)) —
///   0xfff is -1, not 4095;
/// - an op nibble 5-7 or a cmp nibble 6-15 is -ENOSYS, not -EINVAL.
///
/// Split out so the in-kernel suite can pin the ABI decode (the old inline
/// decode masked op with 0xf and read the args unsigned).
pub fn futex_wake_op_decode(encoded: u32) -> Result<(u32, i32, u32, i32), i32> {
    let op = (encoded & 0x7000_0000) >> 28;
    let cmp = (encoded & 0x0f00_0000) >> 24;
    let mut oparg = sign_extend12((encoded & 0x00ff_f000) >> 12);
    let cmparg = sign_extend12(encoded & 0x0000_0fff);
    if encoded & (FUTEX_OP_OPARG_SHIFT << 28) != 0 {
        // Reference: oparg &= 31 after a ratelimited complaint when out of
        // range; negative oparg can never occur here (12-bit sign-extended
        // values in -2048..2047 — the reference's < 0 check dates from its
        // int-typed decode). 1 << (oparg & 31).
        oparg = 1i32 << (oparg & 31);
    }
    if op > FUTEX_OP_XOR {
        return Err(ENOSYS);
    }
    if cmp > crate::sync::futex::FUTEX_OP_CMP_GE {
        return Err(ENOSYS);
    }
    Ok((op, oparg, cmp, cmparg))
}

/// sign_extend32(v, 11) of the reference: 12-bit field to a signed i32.
const fn sign_extend12(v: u32) -> i32 {
    ((v << 20) as i32) >> 20
}

/// FUTEX_WAKE_OP implementation (Linux futex_wake_op).
///
/// Atomically (w.r.t. futex waiters on `uaddr2`) applies the encoded
/// operation to `*uaddr2`, wakes up to `nr_wake` waiters on `uaddr`, and
/// wakes up to `nr_wake2` waiters on `uaddr2` if the comparison holds.
///
/// Encoding (val3): `(op << 28) | (cmp << 24) | (oparg << 12) | cmparg`.
pub fn futex_wake_op(
    uaddr: usize,
    flags: u32,
    nr_wake: i32,
    nr_wake2: i32,
    uaddr2: usize,
    encoded: u32,
) -> i64 {
    // Reference errno shape: an invalid op/cmp nibble is -ENOSYS
    // (futex_atomic_op_inuser's default cases), not -EINVAL.
    let (op, oparg, cmp, cmparg) = match futex_wake_op_decode(encoded) {
        Ok(t) => t,
        Err(e) => return -(e as i64),
    };

    let mm = current_futex_mm_id();
    let key2 = FutexKey::new_user(uaddr2, mm, flags);
    let bucket2 = futex_hash(&key2);

    // Perform the user-word operation while holding uaddr2's bucket lock:
    // a concurrent futex_wait on uaddr2 either re-reads the word under
    // this lock (sees the new value) or is already queued when we wake.
    // Note: the read-modify-write of the user word itself is NOT atomic
    // against userspace amo instructions (no kernel-side user-atomic op
    // here); WAKE_OP users in practice pair it with their own atomics.
    let cmp_holds;
    {
        let _head2 = HASH_HEADS[bucket2].lock_irqsave();
        // SAFETY: exception-table protected access; EFAULT on bad pointer.
        let old = match unsafe {
            crate::arch::uaccess::get_user(uaddr2 as *const u32)
        } {
            Some(v) => v,
            None => return -EFAULT as i64,
        };
        let old_i = old as i32;
        let new: u32 = match op {
            FUTEX_OP_SET => (oparg) as u32,
            FUTEX_OP_ADD => old_i.wrapping_add(oparg) as u32,
            FUTEX_OP_OR => old | (oparg as u32),
            FUTEX_OP_ANDN => old & !(oparg as u32),
            FUTEX_OP_XOR => old ^ (oparg as u32),
            _ => unreachable!("op range checked in futex_wake_op_decode"),
        };
        cmp_holds = match cmp {
            FUTEX_OP_CMP_EQ => old_i == cmparg,
            FUTEX_OP_CMP_NE => old_i != cmparg,
            FUTEX_OP_CMP_LT => old_i < cmparg,
            FUTEX_OP_CMP_LE => old_i <= cmparg,
            FUTEX_OP_CMP_GT => old_i > cmparg,
            FUTEX_OP_CMP_GE => old_i >= cmparg,
            _ => unreachable!("cmp range checked in futex_wake_op_decode"),
        };
        if unsafe {
            !crate::arch::uaccess::put_user(uaddr2 as *mut u32, new)
        } {
            return -EFAULT as i64;
        }
    }

    // The user-word RMW above happens under uaddr2's bucket lock so a
    // concurrent futex_wait on uaddr2 cannot miss it; the actual wakes go
    // through futex_wake, which carries the C8 waker-side invariant
    // (extract task → set `woken` → Task::wake_up).
    let mut woken = futex_wake(uaddr, flags, nr_wake, FUTEX_BITSET_MATCH_ANY);
    if cmp_holds && nr_wake2 > 0 {
        let woken2 = futex_wake(uaddr2, flags, nr_wake2, FUTEX_BITSET_MATCH_ANY);
        if woken >= 0 && woken2 >= 0 {
            woken += woken2;
        }
    }
    woken
}

/// DFX: dump the live state of a futex word + its waiter chain.
#[cfg(feature = "dfx-futex-trace")]
pub fn dfx_dump_futex_state(uaddr: usize, parked_pid: u32) {
    use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
    use crate::dfx::taskdump::dump_syscall_ring_for;
    let bucket = uaddr % HASH_SIZE;
    taskdump_raw_line(b"FTX-STATE u=");
    taskdump_dec(uaddr as u64);
    // The user word and its neighbours (16 words starting uaddr-16).
    for i in 0..4 {
        let a = uaddr + 16 * i;
        match unsafe { crate::arch::uaccess::get_user(a as *const u32) } {
            Some(v) => {
                taskdump_raw_line(b" +");
                taskdump_dec(i as u64 * 4);
                taskdump_raw_line(b"=");
                taskdump_dec(v as u64);
            }
            None => {
                taskdump_raw_line(b" EFAULT");
                break;
            }
        }
    }
    // The bucket chain.
    taskdump_raw_line(b" bucket=");
    taskdump_dec(bucket as u64);
    taskdump_raw_line(b" chain:");
    let head = HASH_HEADS[bucket].lock_irqsave();
    let mut idx = *head;
    let mut n = 0;
    while let Some(i) = idx {
        let (t, ua, wk) = {
            let slot = WAITER_POOL[i].lock_irqsave();
            match slot.as_ref() {
                Some(w) => (w.task, w.key.uaddr, w.woken),
                None => (core::ptr::null_mut(), 0, false),
            }
        };
        taskdump_raw_line(b" [slot=");
        taskdump_dec(i as u64);
        if !t.is_null() {
            taskdump_raw_line(b" pid=");
            taskdump_dec(unsafe { (*t).pid() as u64 });
        }
        taskdump_raw_line(b" u=");
        taskdump_dec(ua as u64);
        if wk {
            taskdump_raw_line(b" WOKEN");
        }
        taskdump_raw_line(b"]");
        idx = {
            let slot = WAITER_POOL[i].lock_irqsave();
            slot.as_ref().and_then(|w| w.next)
        };
        n += 1;
        if n > 8 {
            taskdump_raw_line(b" ...");
            break;
        }
    }
    drop(head);
    taskdump_raw_line(b"\n");
    // Also replay this task's recent syscalls (completed ones) from the
    // forensic ring — the parked wait never records, so the tail shows
    // exactly what led up to it. One-shot: the ring is global/boot-long.
    static RING_DUMPED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    if !RING_DUMPED.swap(true, core::sync::atomic::Ordering::Relaxed) {
        dump_syscall_ring_for(parked_pid);
    }
}

/// do_futex - main dispatch function
pub fn do_futex(uaddr: usize, op: i32, val: u32, _timeout: u64, uaddr2: usize, _val2: u32, val3: u32) -> i64 {
    let flags = futex_to_flags(op as u32);
    let cmd = op & FUTEX_CMD_MASK;

    // All futex words must be 4-byte aligned (Linux get_futex_key).
    if uaddr & 0x3 != 0 {
        return -EINVAL as i64;
    }

    // sc185 RWFRZ-WATCH: any futex op on a word that RWFRZ-PARK named (a
    // writer parked there on 0x80000000|N readers). This is the birth-
    // moment lifecycle: a WAKE arriving on the word while the parked
    // writer never resumes = kernel lost the wake; total silence = the
    // readers never released (userspace leak/stall); a re-issued WAIT
    // from the parked tid = it DID wake once and re-parked (val still
    // nonzero — reader side stuck). Budgeted at 64 lines total.
    {
        use core::sync::atomic::Ordering::Relaxed;
        if RWFRZ_WATCH[0].load(Relaxed) != 0 || RWFRZ_WATCH[1].load(Relaxed) != 0 {
            for w in RWFRZ_WATCH.iter() {
                let wa = w.load(Relaxed);
                if wa != 0 && wa == uaddr {
                    let c = RWFRZ_WATCH_CNT.fetch_add(1, Relaxed);
                    if c < RWFRZ_WATCH_BUDGET {
                        let (tid, tgid) = match crate::sched::current() {
                            Some(t) => unsafe { ((*t).pid(), (*t).tgid()) },
                            None => (0, 0),
                        };
                        crate::pr_warn!(
                            "RWFRZ-WATCH #{} tid={} tgid={} op={} uaddr={:#x} val={:#x} jif={}",
                            c,
                            tid,
                            tgid,
                            match cmd {
                                FUTEX_WAIT => "WAIT",
                                FUTEX_WAIT_BITSET => "WAITB",
                                FUTEX_WAKE => "WAKE",
                                FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => "REQUEUE",
                                _ => "other",
                            },
                            uaddr,
                            val,
                            crate::drivers::timer::get_jiffies(),
                        );
                    }
                    break;
                }
            }
        }
    }

    // sc184 (session R, the screen gate): hdf_devmgr's frozen-rwlock birth
    // tracer. Every stall dump shows devmgr's IPC threads parked in
    // FUTEX_WAIT on a musl-rwlock word reading 0x80000000|N-readers (16
    // and 148 observed) while their binder transactions sit SERVER-OWING —
    // the wedge that keeps render_service's GetService unanswered and SA
    // 10 unpublished (black screen). The word's lifecycle inside devmgr
    // is invisible to every existing probe (uncontended rd/unlock paths
    // never syscall). This tracer prints, for the latched devmgr pid
    // ONLY, every WAIT whose expected value carries the waiters bit (the
    // rwlock writer path — each line's `val` carries the reader count the
    // writer saw) plus every WAKE on an address such a WAIT named,
    // deduped per address so the UART survives: "WAIT-forever preceded by
    // matching WAKEs" vs "no WAKE ever" decides lost-wake vs leaked-reader
    // directly.
    //
    // Serial-cleanliness (print-audit lane, 2026-11): this whole block
    // runs on EVERY futex syscall. Unlatched, it paid a sched::current +
    // comm read + utf8 + name compare per op on every boot that never
    // runs hdf_devmgr; latched (or in the FXDV2 arena window below), it
    // printed up to 900 lines of HEALTHY churn per OH boot (~0.6 s of
    // polled-16550 time). The hunt is gated behind dfx=futextrace (the
    // same switch as the wake-miss scan in futex_wake_in_mm); the
    // sc184/sc185 hunts re-arm with the flag.
    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::FutexTrace)
    {
        use core::sync::atomic::Ordering::Relaxed;
        let cur_tgid =
            crate::sched::current().map(|t| unsafe { (*t).tgid() as u32 });
        // sc185: the SA-5100 parcel latch never fired in sc184 (zero FXDV
        // lines), so also latch by process name — the first futex from any
        // thread of "hdf_devmgr" latches its tgid. The parcel matcher stays
        // (earlier latch = earlier coverage), this is the backstop.
        if FXDV_PID.load(Relaxed) == 0 {
            if let Some(t) = crate::sched::current() {
                let comm = unsafe { (*t).comm() };
                let name = core::str::from_utf8(&comm[..])
                    .unwrap_or("")
                    .trim_end_matches('\0');
                if name == "hdf_devmgr" {
                    fxdv_latch_devmgr(unsafe { (*t).tgid() as u32 });
                }
            }
        }
        if FXDV_PID.load(Relaxed) != 0 && cur_tgid == Some(FXDV_PID.load(Relaxed)) {
            let is_rw_wait = matches!(cmd, FUTEX_WAIT | FUTEX_WAIT_BITSET)
                && val & 0x8000_0000 != 0
                && (val & 0x7fff_ffff) < 0x4000_0000;
            let mut addr_idx = usize::MAX;
            for (i, s) in FXDV_ADDR.iter().enumerate() {
                if s.load(Relaxed) == uaddr {
                    addr_idx = i;
                    break;
                }
            }
            if is_rw_wait && addr_idx == usize::MAX {
                for (i, s) in FXDV_ADDR.iter().enumerate() {
                    if s.load(Relaxed) == 0 {
                        s.store(uaddr, Relaxed);
                        addr_idx = i;
                        break;
                    }
                }
            }
            // 70th (ohw84 decode): the frozen devmgr words carry states no
            // clean musl path produces (a stale EBUSY fast-path marker with
            // every task of the tgid parked as a waiter, and a no-owner
            // 0x80000000 word), and uncontended lock/unlock never syscalls,
            // so the writers are invisible to WAIT/WAKE tracing alone. At
            // the FIRST waiter-bit WAIT from devmgr, arm the sc57 write
            // watchpoint on that word's PAGE (the mutex cluster — the
            // stub/svc/holder locks — shares one page in every observed
            // boot) and on the second distinct word; fuxwatch then logs
            // every write (tid, rip, old->new) from that moment on.
            if is_rw_wait {
                let s1 = FUXWATCH_ADDR.load(Relaxed);
                if s1 == 0 {
                    FUXWATCH_ADDR.store(uaddr as u64, Relaxed);
                    crate::dfx::taskdump::ensure_defer_thread();
                    crate::pr_warn!("FXDV-ARM slot1={:#x} jif={}", uaddr, crate::drivers::timer::get_jiffies());
                } else if (s1 as usize) != uaddr && FUXWATCH2_ADDR.load(Relaxed) == 0 {
                    FUXWATCH2_ADDR.store(uaddr as u64, Relaxed);
                    crate::dfx::taskdump::ensure_defer_thread();
                    crate::pr_warn!("FXDV-ARM slot2={:#x} jif={}", uaddr, crate::drivers::timer::get_jiffies());
                }
                // 70th (ohw86): birth-curve sampler — on every traced devmgr
                // futex, sample slot1's word AND its +0x94 neighbor (the
                // mutex cluster: sem at mutex-0x94 in every observed boot;
                // ohw86 proved the frozen mutex took its last write BEFORE
                // any wait armed the page watch) and print each CHANGE.
                // The final 0->0x10 transition's timestamp brackets the
                // founder to the second, before any watch can arm.
                if let Some(t) = crate::sched::current() {
                    let base = FUXWATCH_ADDR.load(Relaxed) as usize;
                    if base != 0 {
                        static LAST1: [core::sync::atomic::AtomicU32; 2] =
                            [const { core::sync::atomic::AtomicU32::new(u32::MAX) }; 2];
                        for (k, wa) in [base, base.wrapping_add(0x94)].iter().enumerate() {
                            if let Some(w) = unsafe { read_u32_via_task(t, *wa) } {
                                let prev = LAST1[k].load(Relaxed);
                                if w != prev {
                                    LAST1[k].store(w, Relaxed);
                                    crate::pr_warn!(
                                        "FXDV-CURVE word{:+#x} {:#x} -> {:#x} jif={} tid={}",
                                        if k == 0 { 0 } else { 0x94 },
                                        prev,
                                        w,
                                        crate::drivers::timer::get_jiffies(),
                                        unsafe { (*t).pid() }
                                    );
                                }
                            }
                        }
                    }
                }
                // FXDV-WORD: the word's live value at every traced wait —
                // the formation curve (0x10 -> 0x80000010 -> stuck) with
                // the waiter's own expected val beside it decides
                // stale-marker vs racing-writer vs kernel stomp.
                if let Some(t) = crate::sched::current() {
                    if let Some(w) = unsafe { read_u32_via_task(t, uaddr) } {
                        let c = if addr_idx == usize::MAX {
                            0
                        } else {
                            FXDV_CNT[addr_idx].load(Relaxed)
                        };
                        if c < 8 || c % 32 == 0 {
                            crate::pr_warn!(
                                "FXDV-WORD tid={} uaddr={:#x} word={:#x} want={:#x} jif={}",
                                unsafe { (*t).pid() },
                                uaddr,
                                w,
                                val,
                                crate::drivers::timer::get_jiffies()
                            );
                        }
                    }
                }
            }
            let interesting = is_rw_wait
                || (matches!(cmd, FUTEX_WAKE) && addr_idx != usize::MAX && val != 0);
            if interesting {
                let n = FXDV_N.fetch_add(1, Relaxed);
                if n < 400 {
                    // Dedup: first 12 per address, then every 16th (ohw84:
                    // first-3 hid the freeze-formation ordering).
                    let c = if addr_idx == usize::MAX {
                        0
                    } else {
                        FXDV_CNT[addr_idx].fetch_add(1, Relaxed)
                    };
                    if c < 12 || c % 16 == 0 {
                        crate::pr_warn!(
                            "FXDV #{} tid={} op={} uaddr={:#x} val={:#x} jif={}",
                            n,
                            crate::sched::current()
                                .map(|t| unsafe { (*t).pid() })
                                .unwrap_or(0),
                            match cmd {
                                FUTEX_WAIT => "WAIT",
                                FUTEX_WAIT_BITSET => "WAITB",
                                _ => "WAKE",
                            },
                            uaddr,
                            val,
                            crate::drivers::timer::get_jiffies(),
                        );
                    }
                }
            }
        }
        // FXDV2 (sc184 follow-up): the PARAM-ARENA lock tracer. The OH
        // parameter service's shared arena is mapped at the fixed base
        // 0x7ff000000000 (/dev/__parameters__ + fixed-address anon pages —
        // the ww-line's mmap-time naming). Its rwlock word at 0x7ff00002ec64
        // froze with 16 readers in sc184 while writers from param_watcher,
        // el5_filekey_man and storage_manager parked on it — a MAP_SHARED
        // lock whose readers span processes. Trace EVERY futex op in the
        // first 64K of the arena window from ANY process: the WAIT/WAKE
        // lifecycle of the shared lock decides leaked-reader (WAKEs stop
        // while the count is nonzero) vs lost-wake (WAKE issued, writer
        // still parked) vs coherence drift (two processes observe
        // different vals for the same word).
        // GATED behind dfx=futextrace since the lb243 decode: this tracer
        // ran ALWAYS-ON from inside do_futex's hot path, and the param
        // arena is the boot critical path (param_service sync + every
        // param read). Its 500 budgeted console writes (each allocating +
        // 0.36-0.93ms UART) all execute in the first seconds of boot; when
        // the still-unfixed phantom-preempt allocator stall bites
        // mid-print, the printing thread parks INSIDE do_futex with the
        // print half-done (lb243: samgr's self-send worker frozen at
        // do_futex+0x112e over LowerHex/unix_write_slice_owned residue)
        // and the param sync chain freezes the whole service graph — the
        // 'samgr-dark' boot family. Hunt with dfx=futextrace, never by
        // default.
        if (0x7ff0_0000_0000..0x7ff0_0010_0000).contains(&uaddr)
            && crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::FutexTrace)
        {
            use core::sync::atomic::Ordering::Relaxed as R2;
            let n = FXDV2_N.fetch_add(1, R2);
            if n < 500 {
                crate::pr_warn!(
                    "FXDV2 #{} tgid={} tid={} op={} uaddr={:#x} val={:#x} jif={}",
                    n,
                    crate::sched::current().map(|t| unsafe { (*t).tgid() }).unwrap_or(0),
                    crate::sched::current().map(|t| unsafe { (*t).pid() }).unwrap_or(0),
                    match cmd {
                        FUTEX_WAIT => "WAIT",
                        FUTEX_WAIT_BITSET => "WAITB",
                        FUTEX_WAKE => "WAKE",
                        FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => "REQUEUE",
                        _ => "other",
                    },
                    uaddr,
                    val,
                    crate::drivers::timer::get_jiffies(),
                );
            }
        }
    }

    #[cfg(feature = "dfx-futex-trace")]
    {
        let pid = crate::sched::current().map(|t| unsafe { (*t).pid() as u64 }).unwrap_or(0);
        use crate::dfx::taskdump::{taskdump_dec, taskdump_raw_line};
        taskdump_raw_line(b"FTX ENTER pid=");
        taskdump_dec(pid);
        taskdump_raw_line(b" op=");
        taskdump_raw_line(ftx_opname(op));
        taskdump_raw_line(b" priv=");
        taskdump_dec(((op & FUTEX_PRIVATE_FLAG) != 0) as u64);
        taskdump_raw_line(b" u=");
        taskdump_dec(uaddr as u64);
        taskdump_raw_line(b" val=");
        taskdump_dec(val as u64);
        taskdump_raw_line(b"\n");
    }

    let ret = match cmd {
        FUTEX_WAIT => {
            // R7-A9: plain FUTEX_WAIT keeps a RELATIVE timeout even with
            // FUTEX_CLOCK_REALTIME set (Linux futex_init_timeout adds
            // ktime_get() for cmd == FUTEX_WAIT regardless of the flag);
            // only WAIT_BITSET interprets it absolutely.
            match futex_parse_timeout_ns(_timeout, false, false) {
                Ok(dl) => futex_wait_timeout(uaddr, flags, val, FUTEX_BITSET_MATCH_ANY, dl),
                Err(e) => -(e as i64),
            }
        }
        FUTEX_WAKE => {
            futex_wake(uaddr, flags, val as i32, FUTEX_BITSET_MATCH_ANY)
        }
        FUTEX_WAIT_BITSET => {
            // WAIT_BITSET always interprets timeout as absolute time
            // (CLOCK_MONOTONIC base; FUTEX_CLOCK_REALTIME rebases it).
            match futex_parse_timeout_ns(_timeout, true, (flags & FLAGS_CLOCKRT) != 0) {
                Ok(dl) => futex_wait_bitset(uaddr, flags, val, _timeout, val3, dl),
                Err(e) => -(e as i64),
            }
        }
        FUTEX_WAKE_BITSET => {
            futex_wake_bitset(uaddr, flags, val as i32, val3)
        }
        FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => {
            // _timeout is repurposed as nr_requeue in the futex ABI.
            let nr_requeue = _timeout as i32;
            // Linux: negative nr_wake OR negative nr_requeue is EINVAL
            // (requeue.c: `if (nr_wake < 0 || nr_requeue < 0)`); a
            // negative nr_wake used to silently degrade into
            // "requeue everything" instead.
            if nr_requeue < 0 || (val as i32) < 0 {
                return -EINVAL as i64;
            }
            if uaddr2 == 0 || uaddr2 & 0x3 != 0 {
                return -EINVAL as i64;
            }
            // uaddr2 must be a readable user word (get_futex_key faults).
            if unsafe {
                crate::arch::uaccess::get_user(uaddr2 as *const u32).is_none()
            } {
                return -EFAULT as i64;
            }
            futex_requeue(uaddr, flags, val as i32, nr_requeue, uaddr2, val3, cmd == FUTEX_CMP_REQUEUE)
        }
        FUTEX_WAKE_OP => {
            // ABI: val = nr_wake on uaddr, args[3] (_timeout slot) = nr_wake2
            // on uaddr2, val3 = encoded op/cmp. uaddr2 == NULL needs no
            // special case: the reference runs the op through
            // pagefault-disabled user access and reports EFAULT, which is
            // exactly what futex_wake_op's get_user produces on its own.
            if uaddr2 & 0x3 != 0 {
                return -EINVAL as i64;
            }
            futex_wake_op(uaddr, flags, val as i32, _timeout as i32, uaddr2, val3)
        }
        _ => {
            // PI-related operations not yet supported
            -ENOSYS as i64
        }
    };
    // sc58: record completed WAKE-class ops in the verdict ring (the
    // census dumps the window around stuck parks' births). Includes the
    // error returns — a rejected op is also a "fired but delivered
    // nothing" data point.
    if matches!(
        cmd,
        FUTEX_WAKE | FUTEX_WAKE_BITSET | FUTEX_WAKE_OP | FUTEX_REQUEUE | FUTEX_CMP_REQUEUE
    ) {
        fuxw_push(cmd as u8, uaddr as u64, ret as i32);
    }
    // 76th (launcher lane, dfx=txnhunt): samgr FFRT wake audit. The
    // accountmgr ready-flag gate (ohw106): accountmgr's four SA-listener
    // subscribes post two subscribe-time OnAddSystemAbility notify tasks
    // into samgr's FFRT workHandler queue ~30 ms apart — the FIRST runs
    // (notify sent 26.375 s) and the SECOND never does, silently
    // dropping the BMS ready callback and with it the whole StartUser(46)
    // chain. FFRT dispatches queues via futex wakes on its worker pool;
    // this tap prints every WAKE-class op issued by a samgr thread
    // during the first 70 s so the missing PostTask's wake (and its ret
    // — whether a parked worker was actually found) is visible. Zero
    // cost when dfx=txnhunt is off.
    if matches!(
        cmd,
        FUTEX_WAKE | FUTEX_WAKE_BITSET | FUTEX_WAKE_OP | FUTEX_REQUEUE | FUTEX_CMP_REQUEUE
    ) {
        let sp = crate::ipc::binder::binder_sareg_samgr_pid();
        if sp != 0 {
            let mine = crate::sched::current()
                .map(|t| unsafe { (*t).tgid() == sp })
                .unwrap_or(false);
            if mine
                && crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::TxnHang)
                && crate::drivers::timer::get_jiffies() <= 7000
            {
                static SMW_N: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                if SMW_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 3000 {
                    let tid = crate::sched::current()
                        .map(|t| unsafe { (*t).pid() })
                        .unwrap_or(0);
                    let comm = crate::sched::current()
                        .map(|t| unsafe { (*t).comm() })
                        .map(|c| core::str::from_utf8(c).unwrap_or(""))
                        .unwrap_or("")
                        .trim_end_matches('\0');
                    crate::pr_warn!(
                        "SAMWAKE tid={} comm='{}' cmd={} uaddr={:#x} val={} ret={}",
                        tid,
                        comm,
                        match cmd {
                            FUTEX_WAKE => "WAKE",
                            FUTEX_WAKE_BITSET => "WAKE-bs",
                            FUTEX_WAKE_OP => "WAKE-op",
                            FUTEX_REQUEUE => "REQ",
                            _ => "CMP-REQ",
                        },
                        uaddr,
                        val,
                        ret
                    );
                }
            }
        }
    }
    ret
}

/// sys_futex system call entry point
pub fn sys_futex_handler(args: &[u64; 6]) -> i64 {
    let uaddr = args[0] as usize;
    let op = args[1] as i32;
    let val = args[2] as u32;
    let timeout = args[3];
    let uaddr2 = args[4] as usize;
    let val3 = args[5] as u32;

    // sc87: first-writer-park forensics. A FUTEX_WAIT whose expected value
    // is WAITERS|N-readers with N>=4 is a musl-rwlock WRITER parking on a
    // word that ALREADY claims many readers — the freeze's birth moment.
    // The freeze scan (60 s cadence) always arrives after the fact; this
    // catches the very first park, in the parking thread's own syscall
    // context, where the user stack still carries the full write-lock
    // caller chain. Capped at 16 dumps per boot.
    if matches!(op & FUTEX_CMD_MASK, FUTEX_WAIT | FUTEX_WAIT_BITSET) {
        let readers = val & 0x7fffffff;
        if val & 0x80000000 != 0 && readers >= 4 && readers < 0x40000000 {
            static PARKED_DUMPS: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let n = PARKED_DUMPS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            // dfx=parkdump: each park dump costs hundreds of UART+kmsg
            // lines (hilogd.rd_kmsg eats a vCPU copying them out of
            // /dev/kmsg — the b128/b129 lesson), so the park-dump family
            // is OFF in launcher-milestone boots.
            if n < 16 && crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::ParkDump) {
                if let Some(t) = crate::sched::current() {
                    let tk = unsafe { &*t };
                    let comm = tk.comm();
                    let name = core::str::from_utf8(&comm[..])
                        .unwrap_or("")
                        .trim_end_matches('\0');
                    crate::pr_warn!(
                        "RWFRZ-PARK #{} tid {} comm='{}' uaddr={:#x} val={:#x} (writer parking on {} readers)",
                        n,
                        tk.pid(),
                        name,
                        uaddr,
                        val,
                        readers
                    );
                    // sc185: register the word for RWFRZ-WATCH (post-park
                    // lifecycle from any process — see the watch block in
                    // do_futex's prologue). sc185 lesson: the param-arena
                    // word (0x7ff00002ec64, inside FXDV2's window) churns
                    // healthily all boot and ate the whole 64-line budget
                    // by jif=1502, before the terminal freeze (born
                    // ~jif=1900+). FXDV2 owns the arena window; skip it
                    // here so the budget survives to the wedge window.
                    if !(0x7ff0_0000_0000..0x7ff0_0010_0000).contains(&uaddr) {
                        for w in RWFRZ_WATCH.iter() {
                            if w.load(core::sync::atomic::Ordering::Relaxed) == 0 {
                                w.store(uaddr, core::sync::atomic::Ordering::Relaxed);
                                break;
                            }
                        }
                    }
                    // SAFETY: t is the current task, in its own syscall.
                    unsafe { rwfz_stack_frames(t, "park", 12, 64, false) };
                }
            }
        }
    }
    // sc122 (Session P): the app-child park dump. The KILLDUMP path can
    // never sample the park — SIGKILL lands right after the userspace DFX
    // freeze-dump handshake, so every kill catches the child inside that
    // handshake (getrusage loop / open), never the eternal wait. This
    // hook dumps AT WAIT ENTRY, in the parking thread's own syscall
    // context, for exactly the app-child shape: uaddr on the main-stack
    // top pages (0x7ffffffd0000-0x7ffffffff000 — covers the observed
    // c754/c7f4/cc54/cff4 sites with margin for frame variance) from an
    // app comm, any val (2 seen so far, but do not re-filter on it).
    // rwfz_stack_frames then prints rip/rsp/VMA + the RAW 24-word chain
    // (musl callers + the runtime caller + the timespec at rsp+0x10)
    // + file-backed frames. Budget 6 — the park repeats per child; the
    // first dump already carries the chain.
    if matches!(op & FUTEX_CMD_MASK, FUTEX_WAIT | FUTEX_WAIT_BITSET) {
        if uaddr >= 0x7ffffffd0000 && uaddr < 0x7ffffffff000 {
            if let Some(t) = crate::sched::current() {
                let tk = unsafe { &*t };
                let comm = core::str::from_utf8(&tk.comm()[..])
                    .unwrap_or("")
                    .trim_end_matches('\0');
                let is_app = comm.contains("launcher")
                    || comm.contains("settingsdata")
                    || comm.contains("systemui")
                    || comm.contains("dataability");
                if is_app {
                    static APPPARK_DUMPS: core::sync::atomic::AtomicU32 =
                        core::sync::atomic::AtomicU32::new(0);
                    let n =
                        APPPARK_DUMPS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    // dfx=parkdump: the APP-PARK dump prints the child's
                    // whole lib-region VMA table (APPMAP, ~2.9k lines per
                    // dump in sc179) + fd table + syscall rings — pure
                    // chain forensics, OFF in milestone boots (b128/b129
                    // hilogd.rd_kmsg lesson).
                    if n < 6
                        && crate::dfx::switches::enabled(
                            crate::dfx::switches::DfxSwitch::ParkDump,
                        )
                    {
                        crate::pr_warn!(
                            "APP-PARK #{} tid {} comm='{}' uaddr={:#x} val={:#x} op={:#x}",
                            n,
                            tk.pid(),
                            comm,
                            uaddr,
                            val,
                            op
                        );
                        // sc128 (Session P): the nr-only ALL-syscall ring
                        // (last 96 since sc135, successes included,
                        // newest first) — the final action sequence
                        // between the dlopen cascade and the park.
                        {
                            use core::sync::atomic::Ordering::Relaxed;
                            const RL: usize = crate::process::task::LAST_CALLS_LEN;
                            let cur = tk.last_calls_cur.load(Relaxed);
                            let mut s2 = alloc::string::String::new();
                            use core::fmt::Write;
                            let _ = write!(s2, "APP-PARK calls:");
                            for k in 0..RL {
                                let i = (cur + RL - k) % RL; // newest first
                                let v = tk.last_calls[i].load(Relaxed);
                                if v != 0 {
                                    let _ = write!(s2, " {}", v);
                                }
                            }
                            crate::pr_warn!("{}", s2);
                        }
                        // sc138 (Session Q): the FILTERED init ring — the
                        // 96-entry unfiltered ring above drowns in the
                        // retry loop; this one skips {clock, writev,
                        // connect, main-stack futex} so the child's whole
                        // init since fork fits. initA carries a0 for the
                        // newest 32 (prctl option / mmap addr / futex
                        // uaddr). This is the parked side of the diff;
                        // APP-THR prints the working side.
                        {
                            use core::fmt::Write;
                            use core::sync::atomic::Ordering::Relaxed;
                            const RL: usize = crate::process::task::INIT_CALLS_LEN;
                            const BL: usize = crate::process::task::BIRTH_CALLS_LEN;
                            let cur = tk.init_calls_cur.load(Relaxed);
                            // sc143: 512 entries in chunks of 128 so a
                            // single UART flush cannot gut the whole line.
                            let mut chunk = alloc::string::String::new();
                            let mut shown = 0usize;
                            for k in 0..RL {
                                let i = (cur + RL - k) % RL; // newest first
                                let v = tk.init_calls[i].load(Relaxed);
                                if v != 0 {
                                    if shown % 128 == 0 {
                                        if !chunk.is_empty() {
                                            crate::pr_warn!("{}", chunk);
                                        }
                                        chunk = alloc::string::String::new();
                                        let _ = write!(chunk, "APP-PARK init[{}-]:", shown);
                                    }
                                    let _ = write!(chunk, " {}", v);
                                    shown += 1;
                                }
                            }
                            if !chunk.is_empty() {
                                crate::pr_warn!("{}", chunk);
                            }
                            // newest 96 with a0
                            let mut s3 = alloc::string::String::new();
                            let _ = write!(s3, "APP-PARK initA:");
                            for k in 0..96.min(RL) {
                                let i = (cur + RL - k) % RL;
                                let v = tk.init_calls[i].load(Relaxed);
                                if v != 0 {
                                    let a0 = tk.init_a0[i].load(Relaxed);
                                    let _ = write!(s3, " {}@{:#x}", v, a0);
                                }
                            }
                            crate::pr_warn!("{}", s3);
                            // sc143: the birth window (fork-exit onward),
                            // immune to loop churn.
                            let mut s4 = alloc::string::String::new();
                            let _ = write!(s4, "APP-PARK birth:");
                            for k in 0..BL {
                                let v = tk.birth_calls[k].load(Relaxed);
                                if v != 0 {
                                    let a0 = tk.birth_a0[k].load(Relaxed);
                                    let _ = write!(s4, " {}@{:#x}", v, a0);
                                }
                            }
                            crate::pr_warn!("{}", s4);
                        }
                        // sc125 (Session P): the pre-park FAILING-syscall
                        // tail — musl pthread_create mmaps the thread stack
                        // BEFORE clone, so an ENOMEM mmap means no clone
                        // ever reaches the census. Latched per task.
                        {
                            use core::sync::atomic::Ordering::Relaxed;
                            let cur = tk.last_fails_cur.load(Relaxed);
                            let mut s = alloc::string::String::new();
                            use core::fmt::Write;
                            let _ = write!(s, "APP-PARK fails:");
                            for k in 0..8 {
                                let i = (cur + 8 - 1 - k) % 8; // newest first
                                let v = tk.last_fails[i].load(Relaxed);
                                if v != 0 {
                                    // sc135 (Session Q): a0/a1 name the
                                    // prctl option (nr167) and friends.
                                    let a0 = tk.last_fails_a0[i].load(Relaxed);
                                    let a1 = tk.last_fails_a1[i].load(Relaxed);
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
                            crate::pr_warn!("{}", s);
                        }
                        // SAFETY: t is the current task, in its own syscall.
                        unsafe { rwfz_stack_frames(t, "apppark", 12, 96, false) };
                        // sc139: the call-site-validated caller chain.
                        unsafe { apppark_rets(t) };
                        // sc136 (Session Q): the park chain's caller frame
                        // (raw ustack slot[12] 0x7ff0816ff8a8 family) sits
                        // in a lib VMA never printed by the frames scan
                        // (anon-filtered or fd-closed) — and sc135's only
                        // slot-27 frame line was cut at every UART flush
                        // boundary by interleaved hilog output. Dump the
                        // child's whole lib-region VMA table + open fd
                        // paths in COMPACT one-line-per-VMA form (short
                        // lines survive flush boundaries) so every chain
                        // slot resolves offline against the image libs.
                        {
                            use core::fmt::Write;
                            // sc242: Arc-pin the child's mm (unaccounted
                            // foreign-borrow #GP family — see
                            // rwfz_stack_frames).
                            if let Some(aspace_pin) = tk.address_space_arc() {
                                let aspace = aspace_pin.as_ref();
                                let vmas = aspace.vma_read();
                                for vm in vmas.iter() {
                                    let s0 = vm.start().as_usize();
                                    let e0 = vm.end().as_usize();
                                    // lib region only: musl at 0x3fbf...,
                                    // engine libs/arenas at 0x7ff0-0x7ffb.
                                    let in_lib_region = (0x3f00000000..0x400000000)
                                        .contains(&s0)
                                        || (0x7ff000000000..0x7ffb00000000)
                                            .contains(&s0);
                                    if !in_lib_region {
                                        continue;
                                    }
                                    let fd = vm.file_fd();
                                    let path = if fd >= 0 {
                                        // sc210: the wedge dump can walk a
                                        // just-forked task whose fdtable is
                                        // not set yet — fdtable() would
                                        // panic and kill the boot (kernel
                                        // panic "FdTable not initialized",
                                        // task.rs:3958). Diagnostic walks
                                        // must always be None-safe.
                                        tk.try_fdtable()
                                            .and_then(|fdt| fdt.get_file(fd as usize))
                                            .map(|f| probe_file_path(&f))
                                            .unwrap_or_else(|| alloc::string::String::from("?"))
                                    } else {
                                        alloc::string::String::from("-")
                                    };
                                    let mut v = alloc::string::String::new();
                                    let _ = write!(
                                        v,
                                        "APPMAP {:?} {:#x}-{:#x} off={:#x} fd={} sz={:#x} {}",
                                        comm,
                                        s0,
                                        e0,
                                        vm.offset(),
                                        fd,
                                        vm.file_size() as usize,
                                        path
                                    );
                                    crate::pr_warn!("{}", v);
                                }
                            }
                            // Open fd table: names the connect/writev
                            // socket (hilog? samgr session?) and any lib
                            // fds still alive. sc138: classify each fd by
                            // its FileOps identity (unix/tcp/netlink/
                            // binder/pipe/io_uring/file) — fd3's 10MB
                            // anon_inode mapping in sc137 needs the true
                            // object type named, and unix sockets also
                            // print their bound+peer names.
                            // sc210: fdtable() panics on a task whose
                            // fdtable is not set — the dump must be
                            // None-safe or it kills the boot it dumps
                            // (kernel panic "FdTable not initialized").
                            'fdscan: {
                                let Some(fdt) = tk.try_fdtable() else {
                                    break 'fdscan;
                                };
                            let mut shown = 0u32;
                            for fd in 0..32u32 {
                                if let Some(f) = fdt.get_file(fd as usize) {
                                    let kind = match f.get_ops() {
                                        Some(ops) => {
                                            let p = ops as *const crate::fs::FileOps;
                                            if core::ptr::eq(
                                                p,
                                                &crate::net::unix::UNIX_SOCKET_OPS,
                                            ) {
                                                match crate::net::unix::unix_file_of(
                                                    fd as usize,
                                                ) {
                                                    Some((sock, _)) => {
                                                        let bn = sock
                                                            .bound_name
                                                            .lock()
                                                            .clone()
                                                            .unwrap_or_else(|| {
                                                                alloc::string::String::from(
                                                                    "unbound",
                                                                )
                                                            });
                                                        let peer =
                                                            crate::net::unix::unix_peer_bound_name(
                                                                &sock,
                                                            )
                                                            .unwrap_or_else(|| {
                                                                alloc::string::String::from("-")
                                                            });
                                                        alloc::format!(
                                                            "unix bound={} peer={}",
                                                            bn, peer
                                                        )
                                                    }
                                                    None => "unix".into(),
                                                }
                                            } else if core::ptr::eq(
                                                p,
                                                &crate::net::socket::SOCKET_OPS,
                                            ) {
                                                "inet-socket".into()
                                            } else if core::ptr::eq(
                                                p,
                                                &crate::net::netlink::NETLINK_OPS,
                                            ) {
                                                "netlink".into()
                                            } else if core::ptr::eq(p, &crate::net::raw::RAW_OPS)
                                            {
                                                "raw-socket".into()
                                            } else if core::ptr::eq(
                                                p,
                                                &crate::ipc::binder::BINDER_OPS,
                                            ) {
                                                "binder".into()
                                            } else if core::ptr::eq(
                                                p,
                                                &crate::fs::pipe::PIPE_OPS,
                                            ) {
                                                "pipe".into()
                                            } else if core::ptr::eq(
                                                p,
                                                &crate::fs::memfd::MEMFD_FILE_OPS,
                                            ) {
                                                "memfd".into()
                                            } else if core::ptr::eq(
                                                p,
                                                &crate::io_uring::IO_URING_OPS,
                                            ) {
                                                "io_uring".into()
                                            } else {
                                                "file".into()
                                            }
                                        }
                                        None => "anon".into(),
                                    };
                                    let mut v = alloc::string::String::new();
                                    let _ = write!(
                                        v,
                                        "APPFD {:?} fd={} {} kind={}",
                                        comm,
                                        fd,
                                        probe_file_path(&f),
                                        kind
                                    );
                                    crate::pr_warn!("{}", v);
                                    shown += 1;
                                    if shown >= 16 {
                                        break;
                                    }
                                }
                            }
                            }
                            // sc138 (Session Q): decode round 3.
                            // (a) The park caller's code bytes: raw
                            // ustack slot[12] is the return address into
                            // the caller frame — sc137 resolved it to an
                            // ANONYMOUS exec VMA (JIT stub arena or a
                            // memfd-loaded lib). 24 bytes ending at
                            // slot[12] identify the call instruction
                            // offline; the first 8 bytes of the
                            // containing VMA separate memfd-lib (ELF
                            // magic) from a JIT arena.
                            {
                                // Stack-freed guard (KILLDUMP census race
                                // class — see kernel_stack_top_checked):
                                // rsp=0 falls into the existing skip.
                                let rsp = match tk.kernel_stack_top_checked() {
                                    Some(top) => {
                                        #[cfg(feature = "x86_64")]
                                        {
                                            unsafe {
                                                let regs = (top.wrapping_sub(
                                                    crate::arch::x86_64::pt_regs::PT_REGS_SIZE,
                                                ))
                                                    as *const crate::arch::x86_64::pt_regs::PtRegs;
                                                (*regs).rsp as usize
                                            }
                                        }
                                        #[cfg(feature = "riscv64")]
                                        {
                                            unsafe {
                                                let regs = (top.wrapping_sub(
                                                    crate::arch::riscv64::pt_regs::PT_REGS_SIZE,
                                                ))
                                                    as *const crate::arch::riscv64::pt_regs::PtRegs;
                                                (*regs).sp as usize
                                            }
                                        }
                                        #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
                                        { 0usize }
                                    }
                                    None => 0usize,
                                };
                                if rsp != 0 {
                                    let rd64 = |va: usize| -> Option<u64> {
                                        let lo = unsafe {
                                            read_u32_via_task(t, va)
                                        }?;
                                        let hi = unsafe {
                                            read_u32_via_task(t, va + 4)
                                        }?;
                                        Some(lo as u64 | ((hi as u64) << 32))
                                    };
                                    if let Some(slot12) = rd64(rsp + 8 * 12) {
                                        let mut v = alloc::string::String::new();
                                        let _ = write!(
                                            v,
                                            "APPPK12 {:?} slot12={:#x}",
                                            comm, slot12
                                        );
                                        for k in 0..3 {
                                            match rd64((slot12.wrapping_sub(24) + 8 * k) as usize) {
                                                Some(b) => {
                                                    let _ = write!(v, " b{}={:#018x}", 24 - 8 * k, b);
                                                }
                                                None => {
                                                    let _ = write!(v, " b{}=?", 24 - 8 * k);
                                                }
                                            }
                                        }
                                        // VMA start bytes (ELF check)
                                        if let Some(aspace2) = tk.address_space() {
                                            let vmas2 = aspace2.vma_read();
                                            if let Some(vm) = vmas2.find(
                                                crate::mm::page::VirtAddr::new(
                                                    slot12 as usize,
                                                ),
                                            ) {
                                                let start = vm.start().as_usize();
                                                let _ = write!(
                                                    v,
                                                    " vma={:#x}-{:#x} vmab0=",
                                                    start,
                                                    vm.end().as_usize()
                                                );
                                                let first = rd64(start);
                                                match first {
                                                    Some(b) => {
                                                        let _ = write!(v, "{:#018x}", b);
                                                    }
                                                    None => {
                                                        let _ = write!(v, "?");
                                                    }
                                                }
                                                let elf = first.map_or(false, |w| (w & 0xffff) == 0x457f);
                                                let _ = write!(
                                                    v,
                                                    " {}",
                                                    if elf { "MEMFD-LIB(ELF)" } else { "JIT/anon" }
                                                );
                                            }
                                        }
                                        crate::pr_warn!("{}", v);
                                    }
                                }
                            }
                            // (b) The global unix-socket registry once:
                            // which abstract/filesystem listeners exist at
                            // park time (the appconn hex was unreadable —
                            // strncpy stopped at the abstract NUL).
                            if n == 0 {
                                for line in crate::net::unix::registry_snapshot() {
                                    crate::pr_warn!("{}", line);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    // sc118 decode (Session P): every OH app child parks single-threaded
    // in FUTEX_WAIT on its own main-thread stack at the SAME site
    // (uaddr 0x7fffffffc754 / 0x7fffffffc7f4 — page 0x7fffffffc000; the
    // site is shared across launcher/settingsdata/systemui binaries, so
    // it lives in the common musl/early-runtime startup). The sc117
    // clone-refusal probes never fired, so no clone failed. Remaining
    // hypotheses: (a) the runtime never attempts the worker clone,
    // (b) the worker clone succeeds but the thread dies before signaling,
    // (c) the worker DOES send FUTEX_WAKE on this page and the wake is
    // lost or mismatches. This census logs every futex op on that page
    // from ANY comm (workers rename themselves — OS_FFRT_* etc. — so a
    // comm filter would miss the very wake we hunt). Two lines per op:
    // the entry BEFORE do_futex (a WAIT that parks for minutes must
    // still have printed its arrival) and the result after. A WAKE line
    // printing ->0 while a waiter sits parked is a lost wake; no WAKE
    // at all after a WAIT is never-sent ((a) or (b) — the paired
    // appclone census in sys_clone separates those). Budget 150 lines.
    //
    // Serial-cleanliness (print-audit lane, 2026-11): STK_PAGE is 3
    // pages below the stack top of EVERY x86_64 process, so any
    // main-thread futex on a shallow frame hit this — two pr_warn
    // lines per op through the polled UART until the 400-op budget
    // burned (~0.8 s of serial per boot). The census joins the
    // dfx=futextrace family; the sc118/sc119 hunts re-arm with the
    // flag.
    {
        const STK_PAGE: usize = 0x7fffffffc000;
        if uaddr & !0xfff == STK_PAGE
            && crate::dfx::switches::enabled(
                crate::dfx::switches::DfxSwitch::FutexTrace,
            )
        {
            // Budget raised 150 -> 400 for sc119: the page is 3 pages below
            // the stack top of EVERY x86_64 process, so ANY main-thread
            // futex on a shallow frame burns it — appspawn's own main-loop
            // wait would exhaust 150 before the first app child parks.
            // 400 ops = <=800 lines ~= 7 s of UART across a 15 min boot
            // (vs sc115's 2472-line RWFRZ storm at ~1 s) — affordable to
            // guarantee the census still has budget when children park.
            static NS: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if NS.load(core::sync::atomic::Ordering::Relaxed) < 400 {
                NS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                let comm = crate::sched::current()
                    .map(|t| unsafe { (*t).comm() })
                    .map(|c| core::str::from_utf8(c).unwrap_or(""))
                    .unwrap_or("")
                    .trim_end_matches('\0');
                let tid = crate::sched::current()
                    .map(|t| unsafe { (*t).pid() })
                    .unwrap_or(0);
                crate::pr_warn!(
                    "stkfutex: tid {} op={:#x} uaddr={:#x} val={:#x} comm='{}'",
                    tid,
                    op,
                    uaddr,
                    val,
                    comm
                );
                let r = do_futex(uaddr, op, val, timeout, uaddr2, 0, val3);
                rwfz_op_log(uaddr, op, val, r);
                crate::pr_warn!("stkfutex: tid {} -> {}", tid, r);
                return r;
            }
        }
    }
    // sc209 (leg 14): render_service futex lifecycle tracer (see the
    // FXRS_TGID block near the top of this file). Entry line BEFORE
    // do_futex + result line after, for every WAIT/WAKE/BITSET/REQUEUE op
    // issued by any thread of the latched render_service tgid. Main's
    // park uaddr + whether ANY wake ever targets it decides kernel
    // wake-loss vs the FFRT worker never running the synced task.
    {
        let lt = FXRS_TGID.load(core::sync::atomic::Ordering::Relaxed);
        let word_hit = fxrs_word_watched(uaddr);
        if (lt != 0 || word_hit)
            && matches!(
                op & FUTEX_CMD_MASK,
                FUTEX_WAIT
                    | FUTEX_WAKE
                    | FUTEX_WAIT_BITSET
                    | FUTEX_WAKE_BITSET
                    | FUTEX_CMP_REQUEUE
            )
        {
            let mine = crate::sched::current()
                .map(|t| unsafe { (*t).tgid() == lt })
                .unwrap_or(false);
            if mine || word_hit {
                let n = FXRS_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < FXRS_BUDGET {
                    let tid = crate::sched::current()
                        .map(|t| unsafe { (*t).pid() })
                        .unwrap_or(0);
                    let tgid = crate::sched::current()
                        .map(|t| unsafe { (*t).tgid() })
                        .unwrap_or(0);
                    let comm = crate::sched::current()
                        .map(|t| unsafe { (*t).comm() })
                        .map(|c| core::str::from_utf8(c).unwrap_or(""))
                        .unwrap_or("")
                        .trim_end_matches('\0');
                    let cmd = match op & FUTEX_CMD_MASK {
                        FUTEX_WAIT => "WAIT",
                        FUTEX_WAKE => "WAKE",
                        FUTEX_WAIT_BITSET => "WAIT-bs",
                        FUTEX_WAKE_BITSET => "WAKE-bs",
                        FUTEX_CMP_REQUEUE => "CMP-REQ",
                        _ => "other",
                    };
                    crate::pr_debug!(
                        "FXRS #{} {} tid {} tgid {} comm='{}' uaddr={:#x} val={:#x} u2={:#x} v3={:#x} tmo={:#x}",
                        n,
                        cmd,
                        tid,
                        tgid,
                        comm,
                        uaddr,
                        val,
                        uaddr2,
                        val3,
                        timeout
                    );
                    let r = do_futex(uaddr, op, val, timeout, uaddr2, 0, val3);
                    rwfz_op_log(uaddr, op, val, r);
                    crate::pr_debug!("FXRS #{} -> ret={}", n, r);
                    return r;
                }
            }
        }
    }
    // sc156 (Session R): the SA-501 wedge tracer. sc155 closed the
    // registration question to the registrar side: foundation's safwk
    // loader main thread opens libs 401/4606/4607/3299/3203/3704 then
    // NEVER reaches OpenSo(180) — and safwk's FindAndStartPhaseTasks
    // escapes any hung OnStart through WaitForTasks'
    // condition_variable::wait_for(100s), so the loader can only be
    // permanently stuck if a TIMED futex wait never returns. This census
    // prints every timed WAIT/WAIT_BITSET from foundation/sa_main
    // threads (entry line BEFORE the wait + result line after): a
    // missing "->" line names a never-firing timeout as the wedge. The
    // first two waits also dump the caller's syscall rings. The timespec
    // is decoded so relative(100s)/absolute(now+100s) deadlines are
    // distinguishable offline.
    //
    // Serial-cleanliness (print-audit lane, 2026-11): condition_variable
    // wait_for is a hot per-op path for foundation/sa_main threads —
    // ungated this paid a comm compare per timed WAIT plus up to 240
    // pr_warn lines per OH boot. The census joins the dfx=futextrace
    // family; the sc156 hunt re-arms with the flag.
    if matches!(op & FUTEX_CMD_MASK, FUTEX_WAIT | FUTEX_WAIT_BITSET)
        && timeout != 0
        && crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::FutexTrace)
    {
        if let Some(t) = crate::sched::current() {
            let tk = unsafe { &*t };
            let comm = core::str::from_utf8(&tk.comm()[..])
                .unwrap_or("")
                .trim_end_matches('\0');
            if comm.contains("foundation") || comm.contains("sa_main") {
                static FTW_N: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                let n = FTW_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < 240 {
                    let mut ts = [0u8; 16];
                    let ts_ok = unsafe {
                        crate::arch::uaccess::copy_from_user(
                            ts.as_mut_ptr(),
                            timeout as *const u8,
                            ts.len(),
                        ) == 0
                    };
                    let (sec, nsec) = if ts_ok {
                        (
                            u64::from(ts[0]) | (u64::from(ts[1]) << 8) | (u64::from(ts[2]) << 16)
                                | (u64::from(ts[3]) << 24),
                            u32::from(ts[8]) | (u32::from(ts[9]) << 8)
                                | (u32::from(ts[10]) << 16)
                                | (u32::from(ts[11]) << 24),
                        )
                    } else {
                        (0, 0)
                    };
                    crate::pr_warn!(
                        "FTWAIT #{} tid {} comm='{}' uaddr={:#x} val={:#x} op={:#x} ts={}{}.{:09}s",
                        n,
                        tk.pid(),
                        comm,
                        uaddr,
                        val,
                        op,
                        if ts_ok { "" } else { "?" },
                        sec,
                        nsec
                    );
                    // sc156: ring dump on the first two timed waits only —
                    // the wait that never returns carries its caller
                    // context in the rings.
                    if n < 2 {
                        use core::fmt::Write;
                        use core::sync::atomic::Ordering::Relaxed;
                        const RL: usize = crate::process::task::LAST_CALLS_LEN;
                        let cur = tk.last_calls_cur.load(Relaxed);
                        let mut s2 = alloc::string::String::new();
                        let _ = write!(s2, "FTWAIT calls:");
                        for k in 0..RL {
                            let i = (cur + RL - k) % RL; // newest first
                            let v = tk.last_calls[i].load(Relaxed);
                            if v != 0 {
                                let _ = write!(s2, " {}", v);
                            }
                        }
                        crate::pr_warn!("{}", s2);
                        let fcur = tk.last_fails_cur.load(Relaxed);
                        let mut s3 = alloc::string::String::new();
                        let _ = write!(s3, "FTWAIT fails:");
                        for k in 0..8 {
                            let i = (fcur + 8 - 1 - k) % 8;
                            let v = tk.last_fails[i].load(Relaxed);
                            if v != 0 {
                                let a0 = tk.last_fails_a0[i].load(Relaxed);
                                let _ = write!(s3, " nr{}=-{} a0={:#x}", v & 0xffff, v >> 16, a0);
                            }
                        }
                        crate::pr_warn!("{}", s3);
                    }
                    let r = do_futex(uaddr, op, val, timeout, uaddr2, 0, val3);
                    rwfz_op_log(uaddr, op, val, r);
                    crate::pr_warn!("FTWAIT #{} -> ret={}", n, r);
                    return r;
                }
            }
        }
    }
    let r = do_futex(uaddr, op, val, timeout, uaddr2, 0, val3);
    rwfz_op_log(uaddr, op, val, r);
    r
}

// ==================== rwlock freeze watch (sc81) ====================
//
// sc76/sc80 terminal shape: samgr's abilityMapLock_ (musl rwlock) shows
// word 0x80000010 — waiters bit + 16 held READ counts — while the tgid
// has 7 live threads, ALL parked on that word in FUTEX_WAIT. Decoded OH
// musl protocol (from /system/lib/ld-musl-x86_64.so.1):
//   word==0            free
//   0 < count < 0x7fffffff   N readers hold the lock
//   word==0x7fffffff  writer holds (bit31 ORed while parked waiters exist)
// Readers NEVER park at a positive count (tryrdlock CASes unconditionally
// unless overflow) — so every thread parked on this word is a WRITER, and
// new read acquisitions still succeed. The wedge is writer starvation:
// the 16 read counts must drain to 0 for any writer to proceed, and with
// only 7 live threads none of them can hold more than a non-recursive
// scope's worth. Either threads exited holding read scopes (leak), the
// count was inflated by corruption, or threads exist that the futex
// waiter graph does not show (parked in non-futex sleeps). This watch
// decides: word deltas (any unlock traffic), futex op traffic on the
// word, per-scan tid-liveness of the tgid (reader exits), and a one-shot
// full-tgid park-stack dump when the freeze holds.

/// Watched word (uaddr); 0 = inactive. The freeze scan publishes it when
/// a cluster of >=3 waiters sits on an rwlock-shaped word.
static RWFRZ_UADDR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
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
static RWFRZ_OPLOG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
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
    let j = crate::drivers::timer::get_jiffies() as u64;
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
        let t = unsafe { crate::sched::find_task_by_pid(pid) };
        if !t.is_null() {
            let tk = unsafe { &*t };
            let comm = tk.comm();
            let name = core::str::from_utf8(&comm[..])
                .unwrap_or("")
                .trim_end_matches('\0');
            let word = unsafe { read_u32_via_task(t, uaddr) };
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
        crate::pr_warn!("RWFRZ: exit ring has NO exits for tgid {} — readers did not die by task unlink", tgid);
    } else {
        crate::pr_warn!("RWFRZ: exit ring for tgid {}:{}", tgid, s);
    }
}

/// sc93 launcher gate: AMS SIGKILLs a lifecycle-timed-out app while the
/// DFX userspace dumper itself fails on that app (every LIFECYCLE_TIMEOUT
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
    crate::sched::for_each_task(|t| {
        if unsafe { (*t).tgid() as u32 } == tgid {
            if crate::process::task::Task::try_pin(t) {
                threads_cell.borrow_mut().push(t);
            }
        }
    });
    let threads = threads_cell.into_inner();
    let mut hdr = alloc::string::String::new();
    let _ = write!(hdr, "KILLDUMP: tgid {} threads={}", tgid, threads.len());
    for &t in threads.iter() {
        let tk = unsafe { &*t };
        let comm = core::str::from_utf8(tk.comm()).unwrap_or("").trim_end_matches('\0');
        let _ = write!(hdr, " [{}:{}]", tk.pid(), comm);
    }
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
        let tk = unsafe { &*t };
        let comm = core::str::from_utf8(tk.comm()).unwrap_or("").trim_end_matches('\0');
        match parked {
            Some((uaddr, bitset)) => {
                let word = unsafe { read_u32_via_task(t, uaddr) };
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
                let regs = (top.wrapping_sub(crate::arch::x86_64::pt_regs::PT_REGS_SIZE))
                    as *const crate::arch::x86_64::pt_regs::PtRegs;
                // SAFETY: diagnostic read of this task's entry frame.
                let r = unsafe { &*regs };
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
                unsafe { kill_dump_wedge(tk, t, r.orig_rax, [r.rdi, r.rsi, r.rdx]) };
            }
        }
        unsafe { rwfz_stack_frames(t, "KILLDUMP", 10, 96, false) };
    }
    // sc244: release the pins taken at collection (may free the last
    // references of the concurrently-dying threads — schedulable
    // context, no locks held).
    for &t in threads.iter() {
        crate::process::task::Task::task_put(t);
    }
}

/// sc97: type name for a File by fops identity (the kernel-image symbol
/// at the ops address names any not listed here, offline via nm).
fn kd_fd_type(file: &crate::fs::File) -> &'static str {
    let ops = match file.get_ops() {
        Some(o) => o as *const crate::fs::FileOps as *const (),
        None => return "no-ops",
    };
    let is = |p: &crate::fs::FileOps| ops == (p as *const _ as *const ());
    if is(&crate::fs::pipe::PIPE_OPS) {
        return "pipe";
    }
    if is(&crate::fs::fifo::FIFO_OPS) {
        return "fifo";
    }
    if is(&crate::net::unix::UNIX_SOCKET_OPS) {
        return "unix-sock";
    }
    if is(&crate::net::socket::SOCKET_OPS) {
        return "inet-sock";
    }
    if is(&crate::ipc::binder::BINDER_OPS) {
        return "binder";
    }
    if is(&crate::fs::inotify::INOTIFY_OPS) {
        return "inotify";
    }
    if is(&crate::syscall::misc::EVENTFD_OPS) {
        return "eventfd";
    }
    "other"
}

/// sc97: deep dump of the wedged syscall's fd and user arguments.
/// SAFETY: `t` is the live Task whose entry frame the args came from.
unsafe fn kill_dump_wedge(tk: &Task, t: *mut Task, nr: u64, args: [u64; 3]) {
    use core::fmt::Write;
    // sc97 discriminator: RUNNING-state at SIGKILL time means the entry
    // frame's syscall may be STALE (task is executing userspace — e.g. a
    // busy loop) or the task burns kernel time in a non-sleeping loop, or
    // it was set runnable but never linked/scheduled (wake-loss family).
    // utime vs stime names the burn mode; tiny both = never scheduled.
    let linked = match crate::sched::sched::grq_diag_cfs_linked(t) {
        Some(b) => {
            if b { "1" } else { "0" }
        }
        None => "n/a",
    };
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
        let (reads, misses, cycles, fills) = crate::fs::ext4::file::demand_read_stats();
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
                crate::pr_warn!(
                    "KILLDUMP: tid {} fd{} type={} flags={:#x}",
                    tk.pid(),
                    fd,
                    ty,
                    f.flags_bits()
                );
                match ty {
                    "pipe" => {
                        if let Some(pipe) = crate::fs::pipe::pipe_of_file(&f) {
                            let (fill, cap, rc, wc) = pipe.diag_snapshot();
                            let pipe_ptr = unsafe { *f.private_data.get() }
                                .map(|p| p as usize)
                                .unwrap_or(0);
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
                                crate::sched::for_each_task(|ht| {
                                    let hk = unsafe { &*ht };
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
                                                && unsafe { *hf.private_data.get() }
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
                                crate::pr_warn!(
                                    "KILLDUMP: tid {} pipe holders:{}",
                                    tk.pid(),
                                    if holders.is_empty() { " NONE" } else { &holders }
                                );
                            }
                        }
                    }
                    "unix-sock" => {
                        if let Some(us) = crate::net::unix::unix_of_file_diag(&f) {
                            let (qb, qt, cap, ql) = us.diag_snapshot();
                            let peer = us.dgram_peer.lock().clone();
                            let bound = us.bound_name.lock().clone();
                            crate::pr_warn!(
                                "KILLDUMP: tid {} unix kind={:?} bound={:?} dgram_peer={:?} queued={}B truesize={} sndbuf={} qlen={}",
                                tk.pid(),
                                us.kind,
                                bound,
                                peer,
                                qb,
                                qt,
                                cap,
                                ql
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
                let n = unsafe { read_bytes_via_task(t, args[1] as usize, &mut iovs) };
                for i in 0..3 {
                    if n < (i + 1) * 16 {
                        break;
                    }
                    let base = u64::from_le_bytes(iovs[i * 16..i * 16 + 8].try_into().unwrap());
                    let len = u64::from_le_bytes(iovs[i * 16 + 8..i * 16 + 16].try_into().unwrap());
                    let mut head = alloc::vec![0u8; 48];
                    let hn = unsafe { read_bytes_via_task(t, base as usize, &mut head) };
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
            let p = unsafe { read_cstr_via_task(t, args[0] as usize, 96) };
            crate::pr_warn!("KILLDUMP: tid {} open path='{}'", tk.pid(), p);
        }
        165 => {
            // mount(2): source/target/fstype at args[0..2]
            let s = unsafe { read_cstr_via_task(t, args[0] as usize, 96) };
            let tg = unsafe { read_cstr_via_task(t, args[1] as usize, 96) };
            let ft = unsafe { read_cstr_via_task(t, args[2] as usize, 32) };
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

/// Log futex ops on the watched word: every park (with the userspace
/// expected value), every wake (with how many were woken), every wait
/// return. This is the word's entire syscall-visible traffic. Bursts are
/// capped so a pathological loop cannot flood the console.
fn rwfz_op_log(uaddr: usize, op: i32, val: u32, result: i64) {
    if uaddr == 0 || uaddr != RWFRZ_UADDR.load(core::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let n = RWFRZ_OPLOG.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    if n > 128 && n % 16 != 0 {
        return;
    }
    let cmd = match op & FUTEX_CMD_MASK {
        FUTEX_WAIT => "WAIT",
        FUTEX_WAKE => "WAKE",
        FUTEX_WAIT_BITSET => "WAIT-bs",
        FUTEX_WAKE_BITSET => "WAKE-bs",
        FUTEX_CMP_REQUEUE => "CMP-REQ",
        _ => "other",
    };
    let tid = crate::sched::current()
        .map(|t| t.pid() as u64)
        .unwrap_or(0);
    crate::pr_warn!(
        "RWFRZ-OP #{} {} tid {} val={:#x} ret={}",
        n,
        cmd,
        tid,
        val,
        result
    );
}

/// Read up to `buf.len()` bytes in `task`'s user address space via its
/// page tables, stopping at the first unmapped page (KILLDUMP sc97:
/// iovec arrays, path strings, log payload heads of the wedged task).
/// Returns how many bytes were actually read.
/// SAFETY: `task` must be a live Task; read-only diagnostic walk.
unsafe fn read_bytes_via_task(task: *const Task, va: usize, buf: &mut [u8]) -> usize {
    let tk = unsafe { &*task };
    let aspace = match tk.address_space() {
        Some(a) => a,
        None => return 0,
    };
    let root = aspace.root_ppn();
    let mut done = 0usize;
    while done < buf.len() {
        let cur = va + done;
        let (ppn, _) = match unsafe {
            crate::arch::mm::mm_ops::PageTableWalker::walk(root, cur as u64)
        } {
            Some(p) => p,
            None => break,
        };
        let pa = (ppn as usize) << 12;
        let kv = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(pa as u64))
            .bits() as usize
            + (cur & 0xfff);
        let chunk = core::cmp::min(buf.len() - done, 0x1000 - (cur & 0xfff));
        for i in 0..chunk {
            buf[done + i] = unsafe { core::ptr::read_volatile((kv + i) as *const u8) };
        }
        done += chunk;
    }
    done
}

/// Printable snapshot of a NUL-terminated user string in `task`'s space.
/// SAFETY: same contract as read_bytes_via_task.
unsafe fn read_cstr_via_task(task: *const Task, va: usize, max: usize) -> alloc::string::String {
    if va == 0 {
        return alloc::string::String::from("(null)");
    }
    let mut raw = alloc::vec![0u8; max];
    let n = unsafe { read_bytes_via_task(task, va, &mut raw) };
    let end = raw[..n].iter().position(|&b| b == 0).unwrap_or(n);
    let mut s = alloc::string::String::new();
    for &b in &raw[..end] {
        if (0x20..0x7f).contains(&b) {
            s.push(b as char);
        } else {
            s.push('.');
        }
    }
    s
}

/// Read one u32 in `task`'s user address space via its page tables
/// (khungtaskd's get_user would read through the wrong mm).
/// SAFETY: `task` must be a live Task; read-only diagnostic walk.
unsafe fn read_u32_via_task(task: *const Task, va: usize) -> Option<u32> {
    let tk = unsafe { &*task };
    let aspace = tk.address_space()?;
    let root = aspace.root_ppn();
    let (ppn, _) =
        unsafe { crate::arch::mm::mm_ops::PageTableWalker::walk(root, va as u64)? };
    let pa = (ppn as usize) << 12;
    let kv = crate::arch::mm::phys_to_virt(crate::arch::mm::PhysAddr::new(pa as u64)).bits()
        as usize
        + (va & 0xfff);
    Some(unsafe { core::ptr::read_volatile(kv as *const u32) })
}

/// sc138 (Session Q): display path for a File that may be a memfd —
/// `File::path()` prints "anon_inode:[<ino>]" for the pathless files;
/// resolve <ino> through the memfd name registry (recorded at
/// memfd_create) so APPFD/APPMAP can name the engine libs the children
/// map through memfds ("anon_inode:[17] memfd:libXXX").
fn probe_file_path(f: &crate::fs::file::File) -> alloc::string::String {
    let p = f.path();
    if let Some(rest) = p.strip_prefix("anon_inode:[") {
        if let Some(num) = rest.strip_suffix(']') {
            if let Ok(ino) = num.parse::<u64>() {
                if let Some(name) = crate::fs::memfd::memfd_name_by_ino(ino) {
                    return alloc::format!("{} memfd:{}", p, name);
                }
            }
        }
    }
    p
}

/// sc139 (Session Q): call-site-validated stack scan for the parked
/// child. sc138 killed BOTH candidate "caller" slots as stale decoys
/// (slot[1] resolved into libc++ string::assign — data, not code flow;
/// slot[12] points into a zero page), so the cond's true caller chain
/// is still unnamed. This pass keeps a stack qword only when the bytes
/// IMMEDIATELY BEFORE it decode as a call instruction — `E8 rel32`
/// (v-5) or `FF D0-D7` call-reg (v-2) — the cheap x86-64 signature of
/// a real return address. Slots surviving that check are the real
/// frames. SAFETY: `task` live; diagnostic reads through its tables.
unsafe fn apppark_rets(task: *const Task) {
    let tk = unsafe { &*task };
    // Stack-freed guard (KILLDUMP census race class): nothing to walk
    // without a live stack.
    let Some(top) = tk.kernel_stack_top_checked() else {
        return;
    };
    #[cfg(feature = "x86_64")]
    let rsp = unsafe {
        let regs = (top.wrapping_sub(crate::arch::x86_64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::x86_64::pt_regs::PtRegs;
        (*regs).rsp as usize
    };
    #[cfg(not(feature = "x86_64"))]
    let rsp = 0usize;
    if rsp == 0 {
        return;
    }
    // sc242 (leg-17): Arc-pin the foreign task's mm for the whole scan
    // (same unaccounted-borrow #GP family as rwfz_stack_frames).
    let Some(aspace_pin) = tk.address_space_arc() else { return };
    let aspace = aspace_pin.as_ref();
    let vmas = aspace.vma_read();
    let read_u64_at = |va: usize| -> Option<u64> {
        if va & 0xfff > 0xff8 {
            return None;
        }
        let lo = unsafe { read_u32_via_task(task, va)? };
        let hi = unsafe { read_u32_via_task(task, va + 4)? };
        Some(lo as u64 | ((hi as u64) << 32))
    };
    use core::fmt::Write;
    let mut out = alloc::string::String::new();
    let _ = write!(out, "APPPARK-RETS:");
    let mut found = 0u32;
    for i in 0..128u32 {
        if found >= 12 {
            break;
        }
        let Some(v) = read_u64_at(rsp + 8 * i as usize) else {
            continue;
        };
        if v < 0x1000 || v > 0x7fff_ffff_ffff {
            continue;
        }
        // Must land in some VMA (code or JIT — anon allowed here, the
        // call-site check filters garbage).
        let Some(vm) = vmas.find(crate::mm::page::VirtAddr::new(v as usize)) else {
            continue;
        };
        // 8 bytes ending at v: b[7] is the byte at v-1.
        let Some(w) = read_u64_at(v as usize - 8) else {
            continue;
        };
        let b = w.to_le_bytes();
        let is_call = b[3] == 0xE8 /* call rel32 ends at v */
            || (b[6] == 0xFF && (b[7] & 0xF8) == 0xD0) /* call reg */
            || (b[4] == 0xFF && (b[5] & 0xC7) == 0x05); /* call [rip+disp32]-ish */
        if !is_call {
            continue;
        }
        let _ = write!(
            out,
            " [{}]@{:#x}({:#x}-{:#x},off={:#x},b={:#018x})",
            i,
            v,
            vm.start().as_usize(),
            vm.end().as_usize(),
            vm.offset() + (v as usize - vm.start().as_usize()),
            read_u64_at(v as usize).unwrap_or(0)
        );
        found += 1;
    }
    if found == 0 {
        let _ = write!(out, " none");
    }
    crate::pr_warn!("{}", out);
}

/// 71st (70th item 2): RWFRZ dump emission goes through the taskdump
/// focus ring ("rwfz:" class) instead of direct pr_warn. ohw85 showed
/// the direct prints spliced MID-RECORD by the other CPU's census flood
/// (a ret[55]= line cut at 12 hex chars, tail lines lost), so the
/// second-lock caller chain of the frozen-word holder never reached the
/// serial. censusd drains the focus ring single-threaded and first.
/// Splits multi-line strings, chunks each line under the 240-byte
/// printk record cap (cutting at the last space when one is near).
/// `prio` routes to the "rwfza:" ring (drained before everything) —
/// reserved for the ADOPTION dump: ohw90 showed the shared focus class
/// gets flooded by the NBR-PARK mass latch, evicting the founder dump.
fn rwfz_emit_prio(s: &str, prio: bool) {
    const CAP: usize = 200;
    let tag = if prio { "rwfza" } else { "rwfz" };
    for line in s.split('\n') {
        let mut rest = line;
        let mut part = 0u32;
        while !rest.is_empty() {
            let take = core::cmp::min(CAP, rest.len());
            let mut cut = take;
            if cut < rest.len() {
                // Prefer a whitespace boundary within the last 24 bytes.
                if let Some(sp) = rest[..cut].rfind(' ').filter(|&i| i > cut.saturating_sub(24)) {
                    cut = sp;
                }
            }
            crate::dfx::taskdump::defer_emit_line(alloc::format!(
                "{}:{} {}",
                tag,
                part,
                &rest[..cut]
            ));
            rest = &rest[cut..];
            part += 1;
        }
    }
}

fn rwfz_emit(s: &str) {
    rwfz_emit_prio(s, false);
}

/// One user-stack frame line resolved to its covering VMA (Session F's
/// byte-match symbolization method). sc90: `scan_slots` parametrizes the
/// stack window (64 for per-task dumps; the recursion holder needs 256 —
/// its 16-deep chain with OH frame sizes sits ~1-2 KiB above rsp).
/// SAFETY: diagnostic walk of `task`'s tables; `task` live.
unsafe fn rwfz_stack_frames(
    task: *const Task,
    label: &str,
    max_frames: u64,
    scan_slots: u64,
    prio: bool,
) {
    let tk = unsafe { &*task };
    // Stack-freed guard (same class as the KILLDUMP census skip above):
    // rwfz_stack_frames runs on FOREIGN tasks that may have exited and
    // freed their kernel stack — a stackless top is KERNEL_STACK_SIZE and
    // the entry-frame read faults (monkey-b r38 panicked exactly here,
    // rip inside rwfz_stack_frames). Nothing to walk without a stack.
    let Some(top) = tk.kernel_stack_top_checked() else {
        return;
    };
    #[cfg(feature = "x86_64")]
    let (rsp, rip) = {
        let regs = (top.wrapping_sub(crate::arch::x86_64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::x86_64::pt_regs::PtRegs;
        // SAFETY: diagnostic read of this task's entry frame.
        unsafe { ((*regs).rsp, (*regs).rip) }
    };
    #[cfg(feature = "riscv64")]
    let (rsp, rip) = {
        let regs = (top.wrapping_sub(crate::arch::riscv64::pt_regs::PT_REGS_SIZE))
            as *const crate::arch::riscv64::pt_regs::PtRegs;
        // SAFETY: same contract, riscv64.
        unsafe { ((*regs).sp, (*regs).epc) }
    };
    #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
    let (rsp, rip) = (0u64, 0u64);
    // sc242 (leg-17): PIN the mm for the whole dump — the unaccounted
    // `address_space()` borrow of a FOREIGN task lets the victim's
    // concurrent exit_mm free the MmStruct (and its VMA btree) mid-walk
    // (the documented "/proc maps #GP family"), and KILLDUMP runs
    // exactly when the victim's exit is tearing the mm down. sc242
    // #GP'd in the freed VMA tree at the second frame's vma_of lookup.
    let Some(aspace_pin) = tk.address_space_arc() else { return };
    let aspace = aspace_pin.as_ref();
    let vmas = aspace.vma_read();
    // sc90: return the VMA's BASE file offset separately — the loader
    // closes the fd after mmap, leaving fd=-1 on code VMAs that still
    // carry a real mapping offset (sc89: every park rip printed
    // fd=-1 fileoff=0x116dea). Anonymous VMAs have offset 0 AND fd -1,
    // so "offset != 0 || fd >= 0" separates file-backed code frames
    // from heap/stack data pointers.
    let vma_of = |v: u64| -> Option<(usize, usize, i32, usize, usize, u64)> {
        let vm = vmas.find(crate::mm::page::VirtAddr::new(v as usize))?;
        let base_off = vm.offset();
        Some((
            vm.start().as_usize(),
            vm.end().as_usize(),
            vm.file_fd(),
            base_off,
            base_off + (v as usize - vm.start().as_usize()),
            vm.file_size(),
        ))
    };
    let read_u64_at = |va: u64| -> Option<u64> {
        if va & 0xfff > 0xff8 {
            return None;
        }
        let lo = unsafe { read_u32_via_task(task, va as usize)? };
        let hi = unsafe { read_u32_via_task(task, (va + 4) as usize)? };
        Some(lo as u64 | ((hi as u64) << 32))
    };
    // The RIP itself first — the futex park site names the primitive
    // (musl __timedwait caller: rdlock/wrlock/condvar). sc90: ripbytes
    // byte-identify the park-site lib offline (the mapping's fd is gone;
    // filesz prints 0 — offset+bytes are the surviving fingerprints).
    if let Some((s, e, fd, base, off, fsz)) = vma_of(rip) {
        rwfz_emit_prio(&alloc::format!(
            "RWFRZ:   {} rsp={:#x} rip={:#x} vma={:#x}-{:#x} fd={} fileoff={:#x} baseoff={:#x} filesz={:#x} ripbytes={:#018x}",
            label,
            rsp,
            rip,
            s,
            e,
            fd,
            off,
            base,
            fsz as usize,
            read_u64_at(rip).unwrap_or(0)
        ), prio);
    } else {
        rwfz_emit_prio(
            &alloc::format!("RWFRZ:   {} rsp={:#x} rip={:#x} (no vma)", label, rsp, rip),
            prio,
        );
    }
    // sc86: resolve fd -> file PATH for file-backed frames (offline
    // fileoff/byte matching hit missing-lib walls; the path names the
    // library outright). Cached per-fd for the whole dump of this task.
    let mut path_cache: alloc::vec::Vec<(i32, alloc::string::String)> = alloc::vec::Vec::new();
    let path_of = |fd: i32,
                   cache: &mut alloc::vec::Vec<(i32, alloc::string::String)>|
     -> alloc::string::String {
        if let Some((_, p)) = cache.iter().find(|(f, _)| *f == fd) {
            return p.clone();
        }
        let p = tk
            .try_fdtable()
            .and_then(|fdt| fdt.get_file(fd as usize))
            .map(|file| file.path())
            .unwrap_or_else(|| alloc::string::String::from("?"));
        cache.push((fd, p.clone()));
        p
    };
    use core::fmt::Write;
    let mut s = alloc::string::String::new();
    let mut printed = 0u64;
    // sc88: scan deep (callers sat below the 24-slot window in sc84-87)
    // and keep ONLY file-backed candidates — pure heap/stack pointers
    // drowned the signal (every dump showed data pointers, never the
    // code chain). sc90: the filter is `base offset != 0 || fd >= 0`:
    // musl's loader CLOSES the mapping fd after mmap, so loader-mapped
    // code VMAs carry fd=-1 with a real offset (sc89 nbr dump: the
    // holder's only survivors were two fd>=0 DATA-segment pointers —
    // the fd<0 drop had removed the entire code chain).
    for i in 0..scan_slots {
        if printed >= max_frames {
            break;
        }
        let Some(v) = read_u64_at(rsp + 8 * i) else { continue };
        if v < 0x1000 || v > 0x7fff_ffff_ffff {
            continue;
        }
        if let Some((s0, e0, fd, base, off, fsz)) = vma_of(v) {
            if fd < 0 && base == 0 {
                continue; // anonymous (heap/stack): data pointers only
            }
            let bytes = read_u64_at(v).unwrap_or(0);
            let path = if fd >= 0 {
                path_of(fd, &mut path_cache)
            } else {
                alloc::string::String::from("-")
            };
            let _ = write!(
                s,
                "\nRWFRZ:   frame ustack[{}]={:#x} vma={:#x}-{:#x} fd={} fileoff={:#x} baseoff={:#x} filesz={:#x} bytes={:#018x} path={}",
                i, v, s0, e0, fd, off, base, fsz as usize, bytes, path
            );
            printed += 1;
        }
    }
    // sc120 (Session P): the app-child park hunt needs the RAW frame
    // chain — the file-backed-only scan below DROPS return addresses
    // into the loader-mapped musl VMA at 0x3fbf000000 (fd=-1 AND
    // baseoff==0 for the merged first segment: filtered on both
    // clauses), so the musl syscall -> __timedwait -> pthread_primitive
    // -> RUNTIME-caller chain never printed and the only survivors were
    // stale decoys (a libc++ string-assign return at slot 1). 24 raw
    // words = 3 lines: enough to see the whole chain unfiltered.
    {
        use core::fmt::Write;
        let mut raw = alloc::string::String::new();
        let _ = write!(raw, "\nRWFRZ:   {} raw ustack:", label);
        for i in 0..24 {
            if let Some(v) = read_u64_at(rsp + 8 * i) {
                let _ = write!(raw, " {}", v);
            } else {
                break;
            }
        }
        rwfz_emit_prio(&raw, prio);
    }
    // 70th (ohw84): RETS — call-byte-validated return-address scan over a
    // deep window (the apppark_rets method): a slot is a REAL frame only
    // when the 8 bytes ending at it contain a CALL encoding. Raw-window
    // frames proved unreliable (stale decoys from earlier locks in the
    // same stack region decided nothing); the validated chain names the
    // OsalMutexLock callers exactly.
    {
        use core::fmt::Write;
        let mut rets = alloc::string::String::new();
        let _ = write!(rets, "\nRWFRZ:   {} rets:", label);
        let mut found = 0u32;
        for i in 0..96u32 {
            if found >= 14 {
                break;
            }
            let Some(v) = read_u64_at(rsp + 8 * i as u64) else {
                continue;
            };
            if v < 0x1000 || v > 0x7fff_ffff_ffff {
                continue;
            }
            if vmas
                .find(crate::mm::page::VirtAddr::new(v as usize))
                .is_none()
            {
                continue;
            }
            let Some(w) = read_u64_at(v - 8) else {
                continue;
            };
            let b = w.to_le_bytes();
            let is_call = b[3] == 0xE8
                || (b[6] == 0xFF && (b[7] & 0xF8) == 0xD0)
                || (b[4] == 0xFF && (b[5] & 0xC7) == 0x05);
            if !is_call {
                continue;
            }
            if let Some((s0, e0, fd, _base, off, _fsz)) = vma_of(v) {
                let _ = write!(
                    rets,
                    "\nRWFRZ:   {}  ret[{}]={:#x} vma={:#x}-{:#x} fd={} fileoff={:#x} path={}",
                    label,
                    i,
                    v,
                    s0,
                    e0,
                    fd,
                    off,
                    if fd >= 0 {
                        path_of(fd, &mut path_cache)
                    } else {
                        alloc::string::String::from("-")
                    }
                );
                found += 1;
            }
        }
        if found == 0 {
            let _ = write!(rets, " none");
        }
        rwfz_emit_prio(&rets, prio);
    }
    rwfz_emit_prio(&alloc::format!("RWFRZ:   {} frames:{}", label, s), prio);
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
                        let pid = unsafe { (*t).pid() };
                        let live = core::ptr::eq(
                            unsafe { crate::sched::find_task_by_pid(pid) },
                            t,
                        );
                        if live {
                            (
                                Some((w.key.uaddr, w.key.mm, unsafe { (*t).tgid() as u32 }, t)),
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
    // sc83: scanner liveness heartbeat — one line per minute. sc82 lost
    // khungtaskd silently ~2 min after adoption (stall_report's 500 ms
    // irq-off WORLD window); without this line that death is invisible.
    // Serial-cleanliness: the heartbeat joins the wwcensus family so
    // ordinary boots print nothing periodic.
    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::WwCensus) {
        static BEAT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
        let last = BEAT.load(core::sync::atomic::Ordering::Relaxed);
        if now.saturating_sub(last) >= 60 * hz {
            BEAT.store(now, core::sync::atomic::Ordering::Relaxed);
            crate::pr_warn!("RWFRZ: scan alive at {}s", now / hz);
        }
    }
    // ---- adopt / refresh the watched word ----
    let mut adopted = false;
    for c in waiter_clusters(1) {
        let v = match unsafe { read_u32_via_task(c.sample, c.uaddr) } {
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
                        htab[i].lastv = v;
                        htab[i].at = now;
                        crate::pr_warn!(
                            "RWFRZ-HIGH uaddr={:#x} tgid {} waiters={} word={:#x} readers={} (was {}, d{:+})",
                            c.uaddr,
                            c.tgid,
                            c.n,
                            v,
                            cur_r,
                            prev_r,
                            cur_r as i64 - prev_r as i64
                        );
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
                        crate::pr_warn!(
                            "RWFRZ-HIGH uaddr={:#x} tgid {} waiters={} word={:#x} readers={} (first seen)",
                            c.uaddr,
                            c.tgid,
                            c.n,
                            v,
                            readers
                        );
                    }
                }
            }
            drop(hl);
        }
        if adopted || c.n < 3 || !rwlock_frozen_shape(v) {
            continue;
        }
        let cur_u = RWFRZ_UADDR.load(core::sync::atomic::Ordering::Relaxed);
        if cur_u == c.uaddr {
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
        crate::sched::for_each_task(|t| {
            // SAFETY: callback runs under the pid-hash bucket lock (same
            // contract as sleep_snapshot); reads confined to pid fields.
            let tk = unsafe { &*t };
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
            word_now = unsafe { read_u32_via_task(c.sample, c.uaddr) };
            break;
        }
    }
    // ---- word delta ----
    if let Some(v) = word_now {
        let prev = RWFRZ_LASTVAL.load(core::sync::atomic::Ordering::Relaxed);
        if v != prev {
            RWFRZ_LASTVAL.store(v, core::sync::atomic::Ordering::Relaxed);
            RWFRZ_LASTVAL_AT.store(now, core::sync::atomic::Ordering::Relaxed);
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
                    unsafe { read_u32_via_task(c.sample, uaddr + 4) }.unwrap_or(0),
                    unsafe { read_u32_via_task(c.sample, uaddr + 8) }.unwrap_or(0),
                    unsafe { read_u32_via_task(c.sample, uaddr + 12) }.unwrap_or(0),
                    unsafe { read_u32_via_task(c.sample, uaddr + 16) }.unwrap_or(0),
                );
                break;
            }
        }
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
            let v = match unsafe { read_u32_via_task(c.sample, c.uaddr) } {
                Some(v) => v,
                None => continue,
            };
            let r = v & 0x7fffffff;
            if v & 0x80000000 != 0 && r >= 8 && r < 0x40000000 {
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
    crate::sched::for_each_task(|t| {
        // SAFETY: same contract as rwfz_dump_tgid (pid-hash walk, reads
        // confined to the pt-regs entry frame).
        let tk = unsafe { &*t };
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
            let regs = (top.wrapping_sub(
                crate::arch::x86_64::pt_regs::PT_REGS_SIZE,
            )) as *const crate::arch::x86_64::pt_regs::PtRegs;
            // SAFETY: diagnostic read of this task's entry frame.
            unsafe { ((*regs).orig_rax, (*regs).rdi) }
        };
        #[cfg(feature = "riscv64")]
        let (sys, a0) = {
            let regs = (top.wrapping_sub(
                crate::arch::riscv64::pt_regs::PT_REGS_SIZE,
            )) as *const crate::arch::riscv64::pt_regs::PtRegs;
            // SAFETY: same contract, riscv64.
            unsafe { ((*regs).a7, (*regs).a0) }
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
            let w0 = unsafe { read_u32_via_task(t, a0 as usize) }.unwrap_or(0);
            let wp4 = unsafe { read_u32_via_task(t, a0 as usize + 4) }.unwrap_or(0);
            let wm4 = unsafe { read_u32_via_task(t, a0 as usize - 4) }.unwrap_or(0);
            let wp8 = unsafe { read_u32_via_task(t, a0 as usize + 8) }.unwrap_or(0);
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
                let regs = (top.wrapping_sub(
                    crate::arch::x86_64::pt_regs::PT_REGS_SIZE,
                )) as *const crate::arch::x86_64::pt_regs::PtRegs;
                #[cfg(feature = "riscv64")]
                let regs = (top.wrapping_sub(
                    crate::arch::riscv64::pt_regs::PT_REGS_SIZE,
                )) as *const crate::arch::riscv64::pt_regs::PtRegs;
                // SAFETY: diagnostic read of this task's entry frame.
                #[cfg(feature = "x86_64")]
                {
                    unsafe { (*regs).rsp }
                }
                // SAFETY: same contract, riscv64.
                #[cfg(feature = "riscv64")]
                {
                    unsafe { (*regs).sp }
                }
                #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
                {
                    0u64
                }
            };
            let mut line = alloc::string::String::new();
            let mut n = 0u32;
            for i in 0..256u64 {
                let Some(v) = (unsafe { read_u32_via_task(t, (sp + 8 * i) as usize) })
                    .map(|lo| lo as u64)
                    .and_then(|lo| {
                        (unsafe { read_u32_via_task(t, (sp + 8 * i + 4) as usize) })
                            .map(|hi| lo | ((hi as u64) << 32))
                    })
                else {
                    continue;
                };
                if v >= 0x1000 && v <= 0x7fff_ffff_ffff {
                    let _ = write!(line, " {}:{}", i, v);
                    n += 1;
                    if n % 8 == 0 {
                        crate::pr_warn!("RWFRZ: NBR-RAW{}", line);
                        line.clear();
                    }
                }
            }
            if !line.is_empty() {
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
            unsafe { rwfz_stack_frames(t, "nbr", 24, 256, false) };
            unsafe { rwfz_stack_frames(t, "nbr2", 24, 256, false) };
        }
    });
}

/// sc82: one tgid's full park-stack dump (comm/state/syscall/rip + 8
/// byte-verified user-stack frames per thread). Pure diagnostics.
fn rwfz_dump_tgid(tgid: u32, max_tasks: usize, prio: bool) -> usize {
    let count_cell: core::cell::Cell<usize> = core::cell::Cell::new(0);
    crate::sched::for_each_task(|t| {
        // SAFETY: same contract as sleep_snapshot.
        let tk = unsafe { &*t };
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
            let regs = (top.wrapping_sub(
                crate::arch::x86_64::pt_regs::PT_REGS_SIZE,
            )) as *const crate::arch::x86_64::pt_regs::PtRegs;
            // SAFETY: diagnostic read of this task's entry frame.
            unsafe { ((*regs).rip, (*regs).orig_rax, (*regs).rdi) }
        };
        #[cfg(feature = "riscv64")]
        let (rip, sys, a0) = {
            let regs = (top.wrapping_sub(
                crate::arch::riscv64::pt_regs::PT_REGS_SIZE,
            )) as *const crate::arch::riscv64::pt_regs::PtRegs;
            // SAFETY: same contract, riscv64.
            unsafe { ((*regs).epc, (*regs).a7, (*regs).a0) }
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
        unsafe { rwfz_stack_frames(t, "task", 8, 64, prio) };
    });
    count_cell.get()
}
