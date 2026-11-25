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
//! # Futex key semantics (Linux parity — get_futex_key, FWK round)
//! - PRIVATE futexes key on (mm identity, uaddr): the second key component is
//!   the address of the task's `AddressSpace` Arc allocation. All CLONE_VM
//!   threads share one Arc → one key; a forked child COWs a NEW AddressSpace
//!   → different key (exactly Linux's `&mm->mm` key). Kernel threads (no mm)
//!   key on 0.
//! - SHARED-SEMANTIC ops on an ANON word (a PRIVATE VMA — .data, heap,
//!   thread stacks) key on (mm identity, uaddr) as well: reference
//!   `get_futex_key` keys PageAnon words on (current->mm, address)
//!   (FUT_OFF_MMSHARED), never on the physical frame. A frame-keyed anon
//!   word LOSES wakes to fork-COW: fork marks the parent PTE RO+COW, the
//!   next store COW-breaks the page to a fresh frame, and a waiter parked
//!   keyed on the old frame sits in a bucket the waker (resolving the new
//!   frame) never scans — the lostwake.c group-strand (FWK: same-mm
//!   wake-miss, waiter phys 0x7ffc7xxx vs waker phys changing every kill
//!   cycle, 8/8 parkers stuck with a live re-waking waker and zero
//!   scheduler diagnostics).
//! - SHARED-SEMANTIC ops on a SHARED VMA word (MAP_SHARED — the genuine
//!   cross-process case) key on the PHYSICAL word identity
//!   (frame << 12 | offset). Those frames are stable: fork's COW walk
//!   exempts SHARED VMAs (mm_ops.rs cow_exempt), so the frame IS the
//!   shared object (reference: inode/page-index key; Rux's MAP_SHARED
//!   pages are single cache frames aliased in every mapper).

use crate::sync::spinlock::Spinlock;
use core::sync::atomic::AtomicU32;
use crate::process::Task;
use crate::process::task::TaskState;
use crate::errno::constants::{EINVAL, EFAULT, EAGAIN, ENOSYS, ETIMEDOUT};

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

pub static FXRS_TGID: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
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
// rux:fuxstuck (retired debug=fuxstuck)-gated push (one lock+copy per wake op); zero cost off.
#[derive(Clone, Copy)]
pub struct FuxWakeOp {
    pub j: u64,
    pub tid: u32,
    pub tgid: u32,
    pub uaddr: u64,
    pub op: u8, // raw FUTEX cmd byte (FUTEX_WAKE=1 etc.)
    pub ret: i32,
}
pub const FUXW_RING_SZ: usize = 16384; // per CPU: ~6 s of coverage at the OH boot's ~10k wake syscalls/s
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
    if !crate::trace::EVENT_FUXSTUCK.armed() {
        return;
    }
    // 80th (ohw126 AMS park): immediate per-word wake verdict line. The
    // sc58 ring only covers ~6 s of wake traffic; on ohw126 the tid-852
    // park's birth window (77-138 s) was overwritten long before the
    // census dumped it, so the lost-wake-vs-never-sent verdict was
    // unanswerable from the ring. When a fuxwatch slot is armed, every
    // WAKE-class op whose uaddr EQUALS the watched word prints at once:
    //   - wake ret>=1 while the parked waiter stays parked = kernel
    //     wake drop;
    //   - word write (FUXWATCH write log) with NO matching wake line =
    //     userspace never signaled;
    //   - neither = the producer never ran (graph stall, oh68 decode).
    // The watched word is quiet by construction (it is a stall word), so
    // the line volume is small; bounded anyway (the word may be a hot
    // lock before the wedge window).
    {
        let w1 = FUXWATCH_ADDR.load(core::sync::atomic::Ordering::Relaxed);
        let w2 = FUXWATCH2_ADDR.load(core::sync::atomic::Ordering::Relaxed);
        // 81st: the latched sync-holder park word matches too (the word
        // shifts per boot and even mid-park; the census latch tracks it,
        // so the immediate verdict follows the live word). Same-VA
        // foreign objects: the park word is a stack/heap VA mapped in
        // many processes — only the park tgid's ops are the verdict.
        let wp = FUXPARK_WORD.load(core::sync::atomic::Ordering::Relaxed);
        let wp_tgid = FUXPARK_TGID.load(core::sync::atomic::Ordering::Relaxed);
        let w1_tgid = FUXWATCH1_TGID.load(core::sync::atomic::Ordering::Relaxed);
        let cur0 = crate::sched::hotpath::current();
        let cur0_tgid = match cur0 {
            Some(t) => crate::sched::r#unsafe::task_shared(t).tgid() as u32,
            None => 0,
        };
        let m1 = uaddr == w1 && (w1_tgid == 0 || cur0_tgid == w1_tgid);
        let m2 = uaddr == w2;
        let mp = uaddr == wp && (wp_tgid == 0 || cur0_tgid == wp_tgid);
        if (m1 || m2 || mp) && uaddr != 0 {
            static FWW_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            if FWW_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 256 {
                let cur = crate::sched::hotpath::current();
                let (tid2, tgid2, comm) = match cur {
                    Some(t) => {
                        (
                            t.pid(),
                            t.tgid() as u32,
                            core::str::from_utf8(t.comm().as_slice()).unwrap_or(""),
                        )
                    },
                    None => (0, 0, ""),
                };
                let comm = comm.trim_end_matches('\0');
                // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
                crate::pr_warn!(
                    "FUXWATCH-WAKE j={} tid={} tgid={} comm='{}' op={} uaddr={:#x} ret={}",
                    crate::timer::get_jiffies(),
                    tid2,
                    tgid2,
                    comm,
                    cmd,
                    uaddr,
                    ret
                );
            }
        }
    }
    let (tid, tgid) = match crate::sched::hotpath::current() {
        Some(t) => { (t.pid(), t.tgid() as u32) },
        None => (0, 0),
    };
    let r = crate::arch::cpu_id() as usize % FUXW_NRINGS;
    let mut g = FUXW_RINGS[r].lock();
    let (idx, arr) = &mut *g;
    arr[*idx] = FuxWakeOp {
        j: crate::timer::get_jiffies(),
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

const RWFRZ_WATCH_SZ: usize = 8;
pub static RWFRZ_WATCH: [core::sync::atomic::AtomicUsize; RWFRZ_WATCH_SZ] =
    [const { core::sync::atomic::AtomicUsize::new(0) }; RWFRZ_WATCH_SZ];
static RWFRZ_WATCH_CNT: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
// sc186: 64 lines died on arena churn before the wedge window; 192 keeps
// the same worst-case UART bound as one parkdump stack dump.
// glib-mutex: rwfrzbudget=N raises the cap (the garepro repro drives
// thousands of ops/min through the watched word; the default cap goes
// dark long before a freeze could form). Default unchanged.
pub static RWFRZ_WATCH_BUDGET: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(192);

/// rwlock-livelock: tgid whose THREAD EXITS are traced (the exit-path
/// forensics in do_exit read this). Latched at the FIRST plain-mutex
/// contention WAIT (expected val 1/2) from a process whose TGID LEADER
/// comm starts with "udisksd" — early enough to cover exits that happen
/// BEFORE the freeze census adopts the frozen word (the wedge's holder
/// must have died or stalled before the waiter parked).
pub static RWFRZ_EXIT_WATCH_TGID: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

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
    /// Object identity (FWK, reference FUT_OFF_MMSHARED parity): true
    /// only for SHARED-VMA words with a resolved phys — those key (and
    /// bucket) on `phys`, the stable shared object. Every anon word
    /// (PRIVATE VMA, any op flags) carries object=false and keys on
    /// (mm, uaddr): the word's frame is a COW transient (fork re-marks
    /// the PTE RO; the next store swaps the frame), so the frame can
    /// never be the identity of an anon futex.
    pub object: bool,
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
    let Some(task) = crate::sched::hotpath::current() else {
        return 0;
    };

    let walk = |u: usize| -> usize {
        // SAFETY: read-only AddressSpace accessor on the current task.
        let root = match crate::sched::r#unsafe::task_shared(task).address_space() {
            Some(mm) => mm.root_ppn(),
            None => return 0,
        };
        // SAFETY: read-only page-table walk of the current address space.
        match crate::sync::r#unsafe::pt_walk(root, u as u64) {
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
    // AddressSpace accessor; handle_mm_fault operates on the current
    // task's address space, same as the trap path (INV-SCE-2).
    {
        if let Some(mm) = crate::sched::r#unsafe::task_shared(task).address_space() {
            let page = uaddr & !0xfff;
            let _ = crate::api::mm::handle_mm_fault(
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
        Self { uaddr, mm, flags, phys: 0, object: false }
    }

    /// Does the VMA covering `uaddr` in the CURRENT task's address space
    /// carry MAP_SHARED? FWK: only SHARED-VMA words may key on the
    /// physical frame — their frames are exempt from fork's COW marking
    /// and are therefore a stable object identity. Anon words (PRIVATE
    /// VMA) key on (mm, uaddr) whatever their frame currently is (the
    /// reference's PageAnon → FUT_OFF_MMSHARED rule). false on any
    /// lookup failure — the fallback keys (mm, uaddr), which is
    /// self-consistent on both sides of a wake.
    fn vma_is_shared(uaddr: usize) -> bool {
        let task = match crate::sched::hotpath::current() {
            Some(t) => t,
            None => return false,
        };
        // SAFETY: read-only AddressSpace accessor on the current task.
        let mm = match crate::sched::r#unsafe::task_shared(task).address_space() {
            Some(m) => m,
            None => return false,
        };
        let vma = match mm.find_vma(crate::mm::page::VirtAddr::new(uaddr)) {
            Some(v) => v,
            None => return false,
        };
        vma.flags().is_shared()
    }

    /// Production constructor (FWK keying, reference get_futex_key parity):
    /// - PRIVATE op: (mm, uaddr), no page walk, no VMA lookup (the
    ///   reference returns before any GUP) — the hot path gets cheaper.
    /// - SHARED-semantic op: object identity only when the word's VMA is
    ///   MAP_SHARED and the physical word resolved (frame-stable words);
    ///   every anon word keys (mm, uaddr), immune to COW frame swaps.
    pub fn new_user(uaddr: usize, mm: usize, flags: u32) -> Self {
        let k = if flags & FLAGS_SHARED == 0 {
            Self { uaddr, mm, flags, phys: 0, object: false }
        } else {
            let shared_vma = Self::vma_is_shared(uaddr);
            let phys = if shared_vma { resolve_futex_phys(uaddr) } else { 0 };
            Self { uaddr, mm, flags, phys, object: shared_vma && phys != 0 }
        };
        // FXK (r23 setpgid03 hunt): name every futex key construction —
        // uaddr AND the resolved physical identity — so a lost cross-
        // process wake (parent WAIT vs exec'd child WAKE on a MAP_SHARED
        // page) shows which side resolved which frame. Gated behind
        // rux:futextrace (retired debug=futextrace) (console-cost discipline; see FXDV2 note).
        if crate::trace::EVENT_FUTEX_MISS.armed() {
            static FXK_N: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let n = FXK_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            if n < 400 {
                let pid = crate::sched::hotpath::current()
                    .map(|t| t.pid())
                    .unwrap_or(0);
                let comm = crate::sched::hotpath::current()
                    .map(|t| {
                        alloc::string::String::from(t.comm_str())
                    })
                    .unwrap_or_else(|| alloc::string::String::from("?"));
                // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
                crate::pr_warn!(
                    "FXK #{} pid={}({}) uaddr={:#x} phys={:#x} shared={} object={}",
                    n,
                    pid,
                    comm,
                    uaddr,
                    k.phys,
                    k.flags & FLAGS_SHARED != 0,
                    k.object
                );
            }
        }
        k
    }

    /// Check if two keys match
    pub fn matches(&self, other: &FutexKey) -> bool {
        if self.object && other.object {
            // Two object-keyed (SHARED-VMA) keys: physical-word identity
            // (any mapping of the same word matches — cross-address after
            // exec/fresh mmap, the tst_checkpoint protocol).
            self.phys == other.phys
        } else if !self.object && !other.object {
            // Anon/private identity: (mm, uaddr). Covers the musl
            // cleartid interop (shared-semantics waiter vs private
            // waker on the same word of one process) by construction —
            // both key (mm, uaddr) — and is immune to COW frame swaps.
            self.uaddr == other.uaddr && self.mm == other.mm
        } else {
            // Object identity vs (mm, uaddr) identity: different words
            // (reference: inode key never matches a private key).
            false
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
    // Caller guarantees a valid Task (INV-SCE-2); address_space_arc()
    // clones the Arc (bumping the refcount) and we drop it after taking
    // the pointer.
    {
        match crate::sched::r#unsafe::task_shared(task).address_space_arc() {
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
    match crate::sched::hotpath::current() {
        // SAFETY: sched::current() returns the current task's raw pointer,
        // valid for the duration of this syscall.
        Some(t) => task_futex_mm_id(t),
        None => 0,
    }
}

/// DEBUG futex protocol tracer (feature `debug-futex-trace`).
#[cfg(feature = "debug-futex-trace")]
fn ftx_trace(tag: &[u8], uaddr: usize, a: u64, b: u64) {
    use crate::debug::taskdump::{taskdump_dec, taskdump_raw_line};
    let pid = crate::sched::hotpath::current().map(|t| crate::sched::r#unsafe::task_shared(t).pid() as u64).unwrap_or(0);
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

#[cfg(feature = "debug-futex-trace")]
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

#[cfg(not(feature = "debug-futex-trace"))]
fn ftx_trace(_tag: &[u8], _uaddr: usize, _a: u64, _b: u64) {}

/// Waiter information
pub struct Waiter {
    /// Futex key
    pub key: FutexKey,
    /// Waiting task
    pub task: *mut Task,
    /// bitset
    pub bitset: u32,
    /// Whether already woken
    pub woken: bool,
    /// Jiffies when this waiter was linked into its bucket chain (the
    /// park stamp for the rux:fuxstuck (retired debug=fuxstuck) census; 0 on the alloc_waiter
    /// placeholder, overwritten at real insertion). NOT reset on requeue —
    /// the park episode continues, so the original stamp stays truthful.
    pub park_j: u64,
    /// Hash bucket this waiter is currently linked in. Tracked per-waiter
    /// because FUTEX_REQUEUE can move it to uaddr2's bucket while it sleeps;
    /// a stale bucket at removal time silently leaked the slot (review 6.1).
    pub bucket: usize,
    /// Next waiter in hash chain
    pub next: Option<usize>,
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
pub static WAITER_POOL: [crate::sync::lockkey::KeyedSpinlock<
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
pub const HASH_SIZE: usize = crate::config::FUTEX_HASH_SIZE;

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

pub static HASH_HEADS: [crate::sync::lockkey::KeyedSpinlock<
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

pub static FUXWATCH_ADDR: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
// sc57b: second watch slot — the chain walk auto-arms it on the first
// NEW shape-detected lock head that differs from FUXWATCH_ADDR's lock
// (the wedge's nested "B" mutex: the FFRT queue-internal lock).
pub static FUXWATCH2_ADDR: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
static FUXWATCH2_HITS: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
pub static FUXWATCH_DELAY_S: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
static FUXWATCH_HITS: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
// step-marker SET: two CPUs can fault+step concurrently; a single
// marker would lose one thread's #DB → SIGTRAP → thread killed.
static FUXWATCH_STEP_TIDS: [core::sync::atomic::AtomicU32; 4] =
    [const { core::sync::atomic::AtomicU32::new(0) }; 4];
// huks-wedge lane (hw2): the stepped PAGE per marker. The #DB hook used
// to re-protect slot 1's CURRENT page whatever page was stepped — after
// a slot move that W-cleared the WRONG page (an armed-but-idle one) and
// left the stepped page writable. The pair (tid, page) makes the
// retirement land on the page the store actually executed on.
static FUXWATCH_STEP_PAGES: [core::sync::atomic::AtomicU64; 4] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 4];
pub static FUXWATCH_MM_DONE: [core::sync::atomic::AtomicU64; 128] =
    [const { core::sync::atomic::AtomicU64::new(0) }; 128];
const FUXWATCH_BUDGET: usize = 4096;
use core::sync::atomic::Ordering as Relaxed2;
// 81st: when slot 1 was armed by the sync-holder park latch, its word is
// a SINGLE process's object (foundation's main-stack condvar on ohw128:
// 0x7fffffffc304) — but the VA is mapped in EVERY process (main-stack
// page). Arming the page in all 67 mms both floods the exact-word log
// with foreign same-VA writes and (ohw128) overflowed the 16-slot mm
// done-set: mms armed-but-unrecorded took the un-watched fault path on
// their own stack writes -> Permission denied -> SIGSEGV storm ->
// sysrq-c cascade at 97 s. With this set, slot 1's page arms ONLY in
// the latched tgid's mms; foreign same-VA words never fault at all.
pub static FUXWATCH1_TGID: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
// huks-wedge lane (hw1): slot 2 gets its own tgid restriction. The
// 81st restricted only slot 1 (FUXWATCH1_TGID) but arm_round applied
// that restriction to the WHOLE round — with slot 1 tgid-latched, the
// round never visited any other tgid's mms, so a slot-2 word armed by
// the latch (huks on hw1w1) could never actually arm. Slot 2 is now
// tgid-restricted the same way and arm_round visits a task when EITHER
// slot's tgid matches.
pub static FUXWATCH2_TGID: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

// 81st (ohw126 frontier): the sync-holder futex park latch. The 80th's
// immediate per-word wake verdict (fuxw_push) only fires when a fuxwatch
// slot holds the word, but the tid-852 class park word SHIFTS per boot
// (and even mid-park: ohw126 0x7ff082348a84 -> ...fb4 within one page)
// so no fixed cmdline arm can target it. The TXN-HANG-BWR census is the
// first kernel component that SEES the word (a thread parked in FUTEX
// while holding a sync txn — exactly the AMS-looper shape); it latches
// the word here. The latch (a) feeds the immediate wake verdict, (b)
// arms a watch slot for the write side, (c) names the oh68 decode tgid
// (the holder's process — foundation on ohw126, not samgr, so the
// OH67_M2 gate would never decode it).
pub static FUXPARK_WORD: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
pub static FUXPARK_TID: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
pub static FUXPARK_TGID: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
/// Jiffies of the first latch (park age reference for the verdict line).
pub static FUXPARK_J: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);

#[cfg(feature = "x86_64")]
pub fn fuxwatch_retire_page(page: u64) {
    if page == 0 {
        return;
    }
    let va = page & !0xfff;
    let mut touched = 0usize;
    let g = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock();
    for d in FUXWATCH_MM_DONE.iter() {
        let root = d.load(core::sync::atomic::Ordering::Relaxed);
        if root == 0 {
            continue;
        }
        // SAFETY: read-only diagnostic walk of an armed root.
        match crate::sync::r#unsafe::pt_walk(root, va) {
            // Present + user + W-clear = a page WE armed (the arm round
            // wrote 0x1); restore RW.
            Some((_, bits)) if bits & 0x1 != 0 && bits & 0x4 != 0 && bits & 0x2 == 0 => {
                // SAFETY: same live root the arm round modified; one
                // page, RW.
                crate::arch::mm::mm_ops::mprotect_pte_range(root, va, 1, 0x3);
                touched += 1;
            }
            _ => {}
        }
    }
    drop(g);
    if touched > 0 {
        crate::arch::x86_64::mm::asid::flush_tlb_all();
        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
        crate::pr_warn!("FUXWATCH-RETIRE page={:#x} restored W in {} mms", va, touched);
    }
}

/// Non-x86 stub: the W-bit watch is x86_64-only (see fuxwatch_arm_round).
#[cfg(not(feature = "x86_64"))]
pub fn fuxwatch_retire_page(_page: u64) {}

/// huks-wedge lane (hw2): full watch shutdown (budget trip). Retire
/// BOTH slot pages and clear the slot addresses so the fault hook,
/// the #DB hook and the arm rounds all stop matching. After this the
/// instrument is inert and every page it ever touched is writable.
pub fn fuxwatch_retire_all() {
    let a = FUXWATCH_ADDR.load(core::sync::atomic::Ordering::Relaxed);
    let b = FUXWATCH2_ADDR.load(core::sync::atomic::Ordering::Relaxed);
    fuxwatch_retire_page(a);
    fuxwatch_retire_page(b);
    FUXWATCH_ADDR.store(0, core::sync::atomic::Ordering::Relaxed);
    FUXWATCH2_ADDR.store(0, core::sync::atomic::Ordering::Relaxed);
    for d in FUXWATCH_MM_DONE.iter() {
        d.store(0, core::sync::atomic::Ordering::Relaxed);
    }
}

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
        && crate::timer::get_jiffies() < FUXWATCH_DELAY_S.load(Relaxed) * 100
    {
        return;
    }
    crate::sched::hotpath::for_each_task_try(|task| {
        // SAFETY: diagnostic read under the for_each_task contract.
        let tk = crate::sched::r#unsafe::task_shared(task);
        // 81st: a park-latched slot-1 word is watched in its OWN tgid
        // only (see FUXWATCH1_TGID) — the same VA in every other mm is
        // a different object. hw1 (huks-wedge): the restriction is now
        // PER SLOT — slot 2 carries its own tgid (FUXWATCH2_TGID) and
        // the round visits a task when EITHER slot's tgid matches; the
        // 81st applied slot 1's restriction to the whole round, so a
        // slot-2 word latched by the census could never arm.
        let park_tgid1 = FUXWATCH1_TGID.load(Relaxed);
        let park_tgid2 = FUXWATCH2_TGID.load(Relaxed);
        let this_tgid = tk.tgid() as u32;
        let ok1 = park_tgid1 == 0 || this_tgid == park_tgid1;
        let ok2 = park_tgid2 == 0 || this_tgid == park_tgid2;
        if !ok1 && !ok2 {
            return;
        }
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
        for (si, wa) in [&addr, &addr2].iter().enumerate() {
            let wa: u64 = **wa;
            if wa == 0 || to_arm.contains(&wa) {
                continue;
            }
            if si == 0 && !ok1 {
                continue;
            }
            if si == 1 && !ok2 {
                continue;
            }
            let ok = crate::sync::r#unsafe::pt_walk(root, wa)
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
        // 81st (ohw128 SEGV storm): claim a done-slot BEFORE the
        // mprotect — an armed-but-unrecorded mm (done-set overflow: 82
        // mms armed into 16 slots) takes the UN-watched fault path on
        // its own writes -> present-RO PTE -> Permission denied ->
        // SIGSEGV. No free slot: skip the mm whole (never half-arm).
        let mut slot_free = false;
        for d in FUXWATCH_MM_DONE.iter() {
            if d.load(Relaxed) == 0 {
                slot_free = true;
                break;
            }
        }
        if !slot_free {
            return;
        }
        let g = crate::arch::mm::mm_ops::PTE_MODIFY_LOCK.lock();
        // SAFETY: root is a live user page-table root; R-only heap
        // data pages.
        for &wa in to_arm.iter().take(narm) {
            crate::arch::mm::mm_ops::mprotect_pte_range(root,
                    wa & !0xfff,
                    1,
                    0x1);
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
        // HOTPATH-PRINT: one line per armed mm (debug-gated by the param).
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
        for (i, s) in FUXWATCH_STEP_TIDS.iter().enumerate() {
            if s.load(core::sync::atomic::Ordering::Relaxed) == tid {
                // already stepping: refresh the page so a same-tid
                // second fault (signal-handler interleave) retires the
                // page its store will land on
                FUXWATCH_STEP_PAGES[i].store(fault_addr & !0xfff, core::sync::atomic::Ordering::Relaxed);
                return;
            }
        }
        for (i, s) in FUXWATCH_STEP_TIDS.iter().enumerate() {
            if s
                .compare_exchange(0, tid, core::sync::atomic::Ordering::Relaxed, core::sync::atomic::Ordering::Relaxed)
                .is_ok()
            {
                FUXWATCH_STEP_PAGES[i].store(fault_addr & !0xfff, core::sync::atomic::Ordering::Relaxed);
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
        // huks-wedge lane (hw2): the budget used to only stop logging —
        // the arm rounds stopped too, but the page stayed W-cleared, so
        // the NEXT write fell through the (now-skipped) fault hook to
        // do_page_fault → SEGV_ACCERR. Retire the whole watch once: both
        // pages restored, slots cleared — the instrument goes inert
        // instead of arming a trap nobody handles.
        static RETIRED: core::sync::atomic::AtomicBool =
            core::sync::atomic::AtomicBool::new(false);
        if !RETIRED.swap(true, core::sync::atomic::Ordering::Relaxed) {
            // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
            crate::pr_warn!("FUXWATCH budget reached — retiring watch");
            fuxwatch_retire_all();
        }
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
            // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
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
    // when both pages share rips.) 78th: tgid too — every FFRT process
    // allocates a mutexPrivate at the SAME heap VA, so a slot-2 watch
    // traces N processes' objects at once and tid alone cannot
    // attribute a write (ohw115: param_watcher's counter and samgr's
    // mutex shared 0x7ff000499a50).
    let wtgid = crate::sched::hotpath::current()
        .map(|t| crate::sched::r#unsafe::task_shared(t).tgid() as u32)
        .unwrap_or(0);
    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
    crate::pr_warn!("FW tid={} tgid={} rip={:#x} old={} n={} addr={:#x}{}", tid, wtgid, rip, old_word, n, fault_addr, if which2 { " w2" } else { "" });
    // 62nd: latch the owner-of-record on every successful 0->1
    // acquire-class write (the fast-path CAS or the xchg that wins
    // from UNLOCK). old_word==0 with the word transitioning to a
    // locked value = an acquire. After a freeze only old=2 pile-ins
    // remain, so the latch holds the freeze acquirer.
    // 78th: the slot-2 latch is samgr-tgid-gated — the chain armed the
    // slot on SAMGR's mutex word, but the watch itself covers every mm
    // mapping that VA, and foreign acquires (ohw115: param_watcher's
    // old=0 counter write) corrupted the latch and the OH68 owner
    // decode with it.
    if old_word == 0 && (!which2 || {
        let samgr = crate::ipc::binder::binder_sareg_samgr_pid();
        samgr == 0 || wtgid == samgr
    }) {
        FUXWATCH_OWNER_TID.store(tid, core::sync::atomic::Ordering::Relaxed);
        FUXWATCH_OWNER_J
            .store(crate::timer::get_jiffies() as u32, core::sync::atomic::Ordering::Relaxed);
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

/// Read the watched word's CURRENT value through the walker (fault
/// context: the word still holds its pre-write value).
pub fn fuxwatch_read_word(root_ppn: u64) -> u32 {
    let addr = FUXWATCH_ADDR.load(core::sync::atomic::Ordering::Relaxed);
    if addr == 0 {
        return 0xffff_ffff;
    }
    // SAFETY: read-only walk of the current mm.
    match crate::sync::r#unsafe::pt_walk(root_ppn, addr) {
        Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
            let kv = crate::arch::mm::memory_layout::phys_to_virt(
                crate::arch::mm::memory_layout::PhysAddr::new((ppn << 12) | (addr & 0xfff)),
            );
            // SAFETY: linear-mapped RAM word.
            crate::sync::r#unsafe::read_volatile_u32(kv.as_usize())
        }
        _ => 0xffff_ffff,
    }
}

/// #DB hook: Some(page) when this trap is our single step (marker
/// consumed) — the page the stepped store executed on. huks-wedge lane
/// (hw2): the retirement must land on THAT page; re-protecting whichever
/// page slot 1 currently points at (the old behavior) W-clears the wrong
/// page after a slot move and leaves the stepped one writable.
#[inline]
pub fn fuxwatch_take_step(tid: u32) -> Option<u64> {
    for (i, s) in FUXWATCH_STEP_TIDS.iter().enumerate() {
        if s
            .compare_exchange(tid, 0, core::sync::atomic::Ordering::Relaxed, core::sync::atomic::Ordering::Relaxed)
            .is_ok()
        {
            let p = FUXWATCH_STEP_PAGES[i].swap(0, core::sync::atomic::Ordering::Relaxed);
            return Some(p);
        }
    }
    None
}

/// huks-wedge lane (hw2): is this page still watched (either slot)? The
/// #DB retirement only re-protects pages the watch still owns; a page
/// retired by a slot move must stay writable.
#[inline]
pub fn fuxwatch_page_watched(page: u64) -> bool {
    if page == 0 {
        return false;
    }
    let a = FUXWATCH_ADDR.load(core::sync::atomic::Ordering::Relaxed);
    let b = FUXWATCH2_ADDR.load(core::sync::atomic::Ordering::Relaxed);
    (a != 0 && (a & !0xfff) == page) || (b != 0 && (b & !0xfff) == page)
}

/// huks-wedge lane (hw2): read the u32 at `addr` (cr2 precision) through
/// the given root — the fault hook's `old_word` must describe the WORD
/// THAT FAULTED, not slot 1's word (slot-2 writes logged with slot 1's
/// old value before).
pub fn fuxwatch_read_word_at(root_ppn: u64, addr: u64) -> u32 {
    // SAFETY: read-only walk of the current mm.
    match crate::sync::r#unsafe::pt_walk(root_ppn, addr) {
        Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
            let kv = crate::arch::mm::memory_layout::phys_to_virt(
                crate::arch::mm::memory_layout::PhysAddr::new((ppn << 12) | (addr & 0xfff)),
            );
            // SAFETY: linear-mapped RAM word.
            crate::sync::r#unsafe::read_volatile_u32(kv.as_usize())
        }
        _ => 0xffff_ffff,
    }
}

#[inline]
pub fn fuxwatch_addr() -> u64 {
    FUXWATCH_ADDR.load(core::sync::atomic::Ordering::Relaxed)
}

#[inline]
pub fn fuxwatch2_addr() -> u64 {
    FUXWATCH2_ADDR.load(core::sync::atomic::Ordering::Relaxed)
}

/// sc57 (57th): one-shot region scan — a SUSPENDED FFRT coroutine's
/// stack lives in an 8MB stack-pool region (same class as the worker
/// pthread stacks the chain walk already sees); its frames hold the
/// contested lock pointer mid-stack while no thread does. Sweep the
/// top of every large anonymous region of the tgid's mm for the lock
/// range once per boot per lock; print region+offset hits.
/// 61st: single-word read through a task's page tables (walker-only,
/// diagnostic). Shared by the region scan's M3 shape test.
pub fn rd_word(root: u64, va: u64) -> Option<u64> {
    match crate::sync::r#unsafe::pt_walk(root, va) {
        Some((ppn, bits)) if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 => {
            let kv = crate::arch::mm::memory_layout::phys_to_virt(
                crate::arch::mm::memory_layout::PhysAddr::new((ppn << 12) | (va & 0xfff)),
            );
            // SAFETY: linear-mapped RAM word.
            Some(crate::sync::r#unsafe::read_volatile_u64(kv.as_usize()))
        }
        _ => None,
    }
}

/// sc57 (57th): holder scan — for a named lock-object address (the
/// waiter ring's closing head), scan every same-tgid thread's
/// entry-sp window for the lock pointer (the holder's active
/// lock_guard keeps `&lock` spilled in its frame). Walker-only reads
/// of each parked task's frozen mm; debug-gated by the caller. Returns
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

/// g_executeCtxTlsKey VALUE (cached per boot; 0 = unknown).
pub static OH68_KEY: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(0);
/// ExecuteCtx of the latched acquire writer (0 = none).
pub static OH68_OWNER_CTX: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
/// ctx->task at the acquire write (the holder coroutine/thread task).
pub static FUXWATCH_OWNER_TASK: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);
#[cfg(target_arch = "x86_64")]
pub fn oh68_fs_base(tk: &crate::process::Task) -> u64 {
    tk.thread().fs_base
}
#[cfg(not(target_arch = "x86_64"))]
pub fn oh68_fs_base(_tk: &crate::process::Task) -> u64 {
    0
}

/// Resolve a thread's ExecuteCtx from its tsd array: the cached key, or a
/// validated scan of slots 1..64 (ctx->tid == the reading thread's tid).
/// Returns the ctx pointer (0 = not found); caches the key on first hit.
#[cfg(target_arch = "x86_64")]
pub fn oh68_ctx_of(root: u64, fs: u64, tid: u32) -> u64 {
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
pub fn oh68_ctx_of(_root: u64, _fs: u64, _tid: u32) -> u64 {
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
        let Some(tsk) = crate::sched::hotpath::current() else {
            return;
        };
        // SAFETY: current-task accessor in the FW fault path (the same
        // use the trap path itself makes right above this latch's
        // caller).
        let tk = crate::sched::r#unsafe::task_shared(tsk);
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

/// rux:fuxstuck (retired debug=fuxstuck) (17th successor): futex-stuck census, called at the ~1 Hz
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
/// Zero cost when rux:fuxstuck (retired debug=fuxstuck) is off (one switch load).
pub fn futex_stuck_census() {
    use alloc::vec::Vec;
    use core::sync::atomic::Ordering::Relaxed;

    if !crate::trace::EVENT_FUXSTUCK.armed() {
        return;
    }
    let now = crate::timer::get_jiffies();
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
    crate::sched::hotpath::runq_strand_census(now as u64);

    // runq-strand lane (121st): the culprits' side — per-CPU incumbents
    // holding their slot >= 2 s with the last tick's interrupted RIP and
    // mode (kernel loop vs user spin vs halted CPU).
    crate::sched::hotpath::monopolist_census(now as u64);

    // 63rd session (mission lever 2): the FFRT group's wake→pick
    // latency profile per burst — quiet-phase vs wedge-phase cost.
    crate::sched::hotpath::ffrt_wait_census();

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
        uret: [u64; 8],
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
        // 85th (arena founder hunt): VMA decodes for uret[2]/uret[3] —
        // the first frames ABOVE the libc wrapper + its immediate app
        // frame; the +0x8007xx arena parks showed uret[1] still inside
        // libc internals, so the app call site lives at frame 2/3.
        u3v_ok: bool,
        u3v_start: usize,
        u3v_off: usize,
        u4v_ok: bool,
        u4v_start: usize,
        u4v_off: usize,
        // 85th: the park page's structure signature — 8 words at the
        // park page start (allocator/arena headers live there: mallocng
        // group header, ring control block) and 6 words ending at
        // uaddr+0x18 (the wait object's neighborhood: mutex type word,
        // cond seq/mtx pair, list heads).
        pw0: [u64; 8],
        pw0_ok: bool,
        pwu: [u64; 6],
        pwu_ok: bool,
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
    // NO-ALLOC-UNDER-LOCK (oh-launch round 4): Phase A pushes into this
    // Vec while holding each bucket's lock (IRQs off) — a growth there
    // allocates inside the section, the pile-up ingredient the wake
    // paths just lost. The walk caps collection at 192, so reserving
    // that capacity up front (one alloc, no locks held) makes every
    // in-section push capacity-bounded: no allocation under any bucket
    // lock for the whole census.
    let mut stuck: Vec<Stuck> = Vec::with_capacity(192);
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
                // 118th: 96 -> 192 (early-break 192 -> 384) — the wedge
                // population on the oh image is 124+ waiters, and the
                // first-96 bucket-order sample missed exactly the two
                // words the hunt needed (foundation tid 1398's
                // 0x2aab58a77a74 park — only visible via TXN-HANG — and
                // input_user_host's leader word). Same collection-window
                // argument as the 48th; phase-B cost is the O(tasks x
                // stuck) pointer-compare loop.
                if stuck.len() < 192 {
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
                        uret: [0; 8],
                        uret_n: 0,
                        u2v_ok: false,
                        u2v_start: 0,
                        u2v_end: 0,
                        u2v_off: 0,
                        u2v_cls: b'-',
                        u3v_ok: false,
                        u3v_start: 0,
                        u3v_off: 0,
                        u4v_ok: false,
                        u4v_start: 0,
                        u4v_off: 0,
                        pw0: [0; 8],
                        pw0_ok: false,
                        pwu: [0; 6],
                        pwu_ok: false,
                        chn_task: 0,
                        chn_wt: 0,
                        chn_node: 0,
                        chn_hops: [0; 8],
                        chn_hops_ok: 0,
                        chn_prev3: [0; 3],
                        chn_head: 0,
                        chn_head_win: [0; 8],
                    });
                } else if total > 384 {
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
    crate::sched::hotpath::for_each_task_try(|task| {
        for s in stuck.iter_mut() {
            // SAFETY: pointer identity comparison only (no deref of the
            // stored pointer).
            if core::ptr::eq(s.task, task) {
                // SAFETY: diagnostic reads under the for_each_task
                // contract (task pinned in its pid-hash bucket).
                let tk = crate::sched::r#unsafe::task_shared(task);
                s.tgid = tk.tgid() as u32;
                s.tid = tk.pid();
                s.comm.copy_from_slice(tk.comm());
                let mut usp: u64 = 0;
                if let Some(top) = tk.kernel_stack_top_checked() {
                    #[cfg(feature = "x86_64")]
                    {
                        let r = crate::sched::r#unsafe::pt_regs_at_top(top);
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
                        let r = crate::sched::r#unsafe::pt_regs_at_top(top);
                        s.sys = r.a7 as u64;
                        s.epc = r.epc as u64;
                        s.fop = r.a1;
                        s.fval = r.a2;
                        s.fto = r.a3;
                        usp = r.sp;
                    }
                    #[cfg(feature = "aarch64")]
                    {
                        let r = crate::sched::r#unsafe::pt_regs_at_top(top);
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
                            if let Some((ppn, bits)) = crate::sync::r#unsafe::pt_walk(
                                    root,
                                    s.uaddr as u64,
                                ) {
                                if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 {
                                    let kv = crate::arch::mm::memory_layout::phys_to_virt(
                                        crate::arch::mm::memory_layout::PhysAddr::new(
                                            (ppn << 12) | ((s.uaddr & 0xfff) as u64),
                                        ),
                                    );
                                    // SAFETY: linear-mapped RAM word; the
                                    // waiter parked on it this syscall.
                                    let w32 = crate::sync::r#unsafe::read_volatile_u32(kv.as_usize());
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
                                match crate::sync::r#unsafe::pt_walk(root, va) {
                                    Some((ppn, bits))
                                        if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 =>
                                    {
                                        let kv = crate::arch::mm::memory_layout::phys_to_virt(
                                            crate::arch::mm::memory_layout::PhysAddr::new(
                                                (ppn << 12) | (va & 0xfff),
                                            ),
                                        );
                                        // SAFETY: linear-mapped RAM word.
                                        words[wi] = crate::sync::r#unsafe::read_volatile_u64(kv.as_usize());
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
                        // 85th: window 32 -> 64 words, cap 4 -> 8 — the
                        // +0x8007xx arena parks kept uret[1] inside libc
                        // internals; the app call site sits at frame 2/3.
                        if usp != 0 {
                            let root = aspace.root_ppn();
                            let mut va = (usp + 7) & !7u64;
                            for _ in 0..64 {
                                if s.uret_n >= 8 {
                                    break;
                                }
                                // SAFETY: root_ppn of the pinned task's
                                // live mm; walk is read-only.
                                if let Some((ppn, bits)) = crate::sync::r#unsafe::pt_walk(
                                        root, va,
                                    ) {
                                    if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 {
                                        let kv = crate::arch::mm::memory_layout::phys_to_virt(
                                            crate::arch::mm::memory_layout::PhysAddr::new(
                                                (ppn << 12) | (va & 0xfff),
                                            ),
                                        );
                                        // SAFETY: linear-mapped RAM word.
                                        let w = crate::sync::r#unsafe::read_volatile_u64(kv.as_usize());
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
                        // (c1) 85th: VMA decode of uret[2]/uret[3] — the
                        // app-frame candidates above the libc wrapper
                        // (file offset symbolizes offline vs the image).
                        if s.uret_n > 2 {
                            if let Some(vma) = mgr.find(VirtAddr::new(s.uret[2] as usize)) {
                                s.u3v_ok = true;
                                s.u3v_start = vma.start().as_usize();
                                s.u3v_off = s.uret[2] as usize - s.u3v_start;
                            }
                        }
                        if s.uret_n > 3 {
                            if let Some(vma) = mgr.find(VirtAddr::new(s.uret[3] as usize)) {
                                s.u4v_ok = true;
                                s.u4v_start = vma.start().as_usize();
                                s.u4v_off = s.uret[3] as usize - s.u4v_start;
                            }
                        }
                        // (c2) 85th: park-page structure signature. The
                        // +0x8007xx arena-control-page family parks point
                        // at words 0x6xx-0x8xx into an 8MiB+4K anonymous
                        // VMA's LAST page — either a dedicated control
                        // page of a ring arena or a small mapping merged
                        // behind an FFRT CSA arena. 8 words at the park
                        // page start + 6 words around the wait word name
                        // the owning structure (mallocng group header at
                        // page start vs control-block vs object array).
                        if s.uvma_ok {
                            let root = aspace.root_ppn();
                            let pw_base = (s.uaddr & !0xfff) as u64;
                            let mut ok = true;
                            for wi in 0..8usize {
                                let va = pw_base + 8 * (wi as u64);
                                match crate::sync::r#unsafe::pt_walk(root, va) {
                                    Some((ppn, bits))
                                        if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 =>
                                    {
                                        let kv = crate::arch::mm::memory_layout::phys_to_virt(
                                            crate::arch::mm::memory_layout::PhysAddr::new(
                                                (ppn << 12) | (va & 0xfff),
                                            ),
                                        );
                                        // SAFETY: linear-mapped RAM word on
                                        // the parked waiter's own page.
                                        s.pw0[wi] = crate::sync::r#unsafe::read_volatile_u64(kv.as_usize());
                                    }
                                    _ => {
                                        ok = false;
                                        break;
                                    }
                                }
                            }
                            s.pw0_ok = ok;
                            // 6 words ending at uaddr+0x18 (uaddr-0x28 ..
                            // uaddr+0x18): the wait object neighborhood.
                            let ub = (s.uaddr & !0x7) as u64;
                            let mut ok = true;
                            for wi in 0..6usize {
                                let va = ub.wrapping_sub(0x28) + 8 * (wi as u64);
                                match crate::sync::r#unsafe::pt_walk(root, va) {
                                    Some((ppn, bits))
                                        if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 =>
                                    {
                                        let kv = crate::arch::mm::memory_layout::phys_to_virt(
                                            crate::arch::mm::memory_layout::PhysAddr::new(
                                                (ppn << 12) | (va & 0xfff),
                                            ),
                                        );
                                        // SAFETY: linear-mapped RAM word on
                                        // the parked waiter's own page.
                                        s.pwu[wi] = crate::sync::r#unsafe::read_volatile_u64(kv.as_usize());
                                    }
                                    _ => {
                                        ok = false;
                                        break;
                                    }
                                }
                            }
                            s.pwu_ok = ok;
                        }
                        // (c3) 85th: one-shot geometry dump for the stuck
                        // tgid — (i) every anonymous VMA >= 8MiB (the FFRT
                        // CSA arenas are exactly 0x800000; the park VMAs
                        // are 0x801000 — merged? dedicated control page?)
                        // plus the park VMA's immediate neighbors, and
                        // (ii) the executable file-backed map so host-side
                        // symbolization of uret[1..3] works for ANY stuck
                        // pid (FUX-MAPS only ever dumped samgr). Budget:
                        // 6 distinct tgids per boot, 24+48 lines each.
                        if s.uvma_ok && s.uvma_end >= s.uvma_start + 0x800000 {
                            const ARENA_TGID_BUDGET: usize = 6;
                            static ARENA_SEEN: [core::sync::atomic::AtomicU32;
                                ARENA_TGID_BUDGET] = [
                                core::sync::atomic::AtomicU32::new(0),
                                core::sync::atomic::AtomicU32::new(0),
                                core::sync::atomic::AtomicU32::new(0),
                                core::sync::atomic::AtomicU32::new(0),
                                core::sync::atomic::AtomicU32::new(0),
                                core::sync::atomic::AtomicU32::new(0),
                            ];
                            let mut already = false;
                            for slot in ARENA_SEEN.iter() {
                                if slot.load(Relaxed) == s.tgid {
                                    already = true;
                                    break;
                                }
                            }
                            let mut claimed = false;
                            if !already {
                                for slot in ARENA_SEEN.iter() {
                                    if slot
                                        .compare_exchange(
                                            0,
                                            s.tgid,
                                            Relaxed,
                                            Relaxed,
                                        )
                                        .is_ok()
                                    {
                                        claimed = true;
                                        break;
                                    }
                                }
                            }
                            if claimed {
                                use crate::mm::vma::VmaType;
                                // Collect the VMA table (small: < 256
                                // entries per OH process), then select.
                                struct VRow {
                                    vs: usize,
                                    ve: usize,
                                    anon_big: bool,
                                    exec_fb: bool,
                                    off: usize,
                                    fd: i32,
                                }
                                let mut rows: alloc::vec::Vec<VRow> = alloc::vec::Vec::new();
                                for vma in mgr.iter() {
                                    if rows.len() >= 256 {
                                        break;
                                    }
                                    let vs = vma.start().as_usize();
                                    let ve = vma.end().as_usize();
                                    rows.push(VRow {
                                        vs,
                                        ve,
                                        anon_big: matches!(vma.vma_type(), VmaType::Anonymous)
                                            && ve - vs >= 0x800000,
                                        exec_fb: vma.flags().is_executable()
                                            && matches!(vma.vma_type(), VmaType::FileBacked),
                                        off: vma.offset(),
                                        fd: vma.file_fd(),
                                    });
                                }
                                let park_idx = rows
                                    .iter()
                                    .position(|r| r.vs == s.uvma_start && r.ve == s.uvma_end);
                                let mut arena_n = 0usize;
                                let mut exec_n = 0usize;
                                for (i, r) in rows.iter().enumerate() {
                                    let near_park = park_idx.is_some_and(|p| {
                                        i + 1 == p || i == p + 1 || i == p
                                    });
                                    if (r.anon_big || near_park) && arena_n < 24 {
                                        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                                        crate::pr_warn!(
                                            "ARENA-VMAS pid={} {:x}-{:x} sz={:#x} park{}",
                                            s.tgid,
                                            r.vs,
                                            r.ve,
                                            r.ve - r.vs,
                                            if near_park { "*" } else { "" }
                                        );
                                        arena_n += 1;
                                    }
                                    if r.exec_fb && exec_n < 48 {
                                        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                                        crate::pr_warn!(
                                            "ARENA-EXEC pid={} {:x}-{:x} off={:#x} fd={}",
                                            s.tgid,
                                            r.vs,
                                            r.ve,
                                            r.off,
                                            r.fd
                                        );
                                        exec_n += 1;
                                    }
                                }
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
                                match crate::sync::r#unsafe::pt_walk(root, va) {
                                    Some((ppn, bits))
                                        if bits & 1 != 0 && bits & 0x4 != 0 && ppn != 0 =>
                                    {
                                        let kv = crate::arch::mm::memory_layout::phys_to_virt(
                                            crate::arch::mm::memory_layout::PhysAddr::new(
                                                (ppn << 12) | (va & 0xfff),
                                            ),
                                        );
                                        // SAFETY: linear-mapped RAM word.
                                        Some(crate::sync::r#unsafe::read_volatile_u64(kv.as_usize()))
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
    crate::sched::hotpath::for_each_task_try(|task| {
        // SAFETY: diagnostic reads under the for_each_task contract.
        let tk = crate::sched::r#unsafe::task_shared(task);
        let tgid = tk.tgid() as u32;
        if !stuck.iter().any(|s| s.tgid == tgid) {
            return;
        }
        let st_bits = tk.state().bits();
        let run = st_bits == crate::process::task::TaskState::RUNNING
            || tk.on_cpu()
            || tk.sched_entity().is_on_rq();
        let fux = tk.ext.futex_wait_enqueued.load(Relaxed);
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
        rep_uret: [u64; 8],
        rep_uret_n: usize,
        rep_u2v_ok: bool,
        rep_u2v_start: usize,
        rep_u2v_end: usize,
        rep_u2v_off: usize,
        rep_u2v_cls: u8,
        // 85th: app-frame decodes + park-page structure signature.
        rep_u3v_ok: bool,
        rep_u3v_start: usize,
        rep_u3v_off: usize,
        rep_u4v_ok: bool,
        rep_u4v_start: usize,
        rep_u4v_off: usize,
        rep_pw0: [u64; 8],
        rep_pw0_ok: bool,
        rep_pwu: [u64; 6],
        rep_pwu_ok: bool,
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
                        rep_u3v_ok: s.u3v_ok,
                        rep_u3v_start: s.u3v_start,
                        rep_u3v_off: s.u3v_off,
                        rep_u4v_ok: s.u4v_ok,
                        rep_u4v_start: s.u4v_start,
                        rep_u4v_off: s.u4v_off,
                        rep_pw0: s.pw0,
                        rep_pw0_ok: s.pw0_ok,
                        rep_pwu: s.pwu,
                        rep_pwu_ok: s.pwu_ok,
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
        // rux:fuxstuck (retired debug=fuxstuck) at futex_stuck_census() entry plus the 5s burst
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
        // 85th: frame-2/3 decodes (app call-site candidates).
        if g.rep_u3v_ok {
            let _ = write!(u2v_s, " u3v={:#x}+{:#x}", g.rep_u3v_start, g.rep_u3v_off);
        }
        if g.rep_u4v_ok {
            let _ = write!(u2v_s, " u4v={:#x}+{:#x}", g.rep_u4v_start, g.rep_u4v_off);
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
        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
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
        // debug-migrate: the census row also records into the unified
        // framework (rux:fuxstuck — the event is also the family's arm
        // switch, so it is armed by construction here). The record
        // carries the row's identifying fields; the serial line above
        // keeps the full context dump.
        crate::trace::trace_fuxstuck(
            g.tgid,
            g.rep_tid,
            g.uaddr as u64,
            g.rep_mm as u64,
            g.n as u32,
            g.min_age,
            g.max_age,
            g.rep_sys as i32,
            g.rep_epc,
        );
        // neverword lane (forensic-only, not for main): EventRunner-named
        // standing park >= 60 s gets the CTD decode UNCONDITIONALLY. The
        // lr-series f2 boot proved the W-trigger's 2-per-burst budget is
        // consumed every burst by older standing words (storage_daemon
        // parked 8 s, bluetooth 12 s ...) iterated ahead of foundation's
        // FRESH-tail group (parked 10.5 s): CTD-END W fired for tgids
        // 421/438/637/641 only, never for foundation 709 — the launcher
        // wall's word never decoded. The DONE latch inside ctd_decode
        // makes this at-most-once per tgid per boot.
        if is_fresh
            && g.max_age >= 60
            && g.uaddr > 0x1000
            && comm_s.starts_with("EventRunner")
        {
            crate::api::sync::ctd_decode(g.tgid, g.uaddr as u64, "V");
        }
        // 118th (mouse-endgame): standing-word holder scan. A group whose
        // park has stood >= 60 s names a lock that will never turn again;
        // the TXN-HANG parkv arm only latches rwfrz_reader_scan for looper
        // threads owing a txn, so the empty-stack parks (foundation tid
        // 1332 on the 0x80000010 mutex; the HDI host leader parks at
        // fval 0x80000000) never got a holder verdict. Run the existing
        // reader scan for THIS word in THIS tgid — every same-tgid thread
        // whose stack window references the word prints as an RDR row
        // with its own park word (the holder's terminal park; a zero-hit
        // pass acquits thread-side holders and points at a suspended
        // coroutine). Once per (tgid, word) per 60 s, budget 2 per burst
        // — the same for_each_task_try + page-walk cost class the parkv
        // arm already runs in this heartbeat slot.
        if g.max_age >= 60 && g.uaddr > 0x1000 {
            use core::sync::atomic::Ordering::Relaxed;
            static SCAN_TGID: [core::sync::atomic::AtomicU32; 24] =
                [const { core::sync::atomic::AtomicU32::new(0) }; 24];
            static SCAN_WORD: [core::sync::atomic::AtomicU64; 24] =
                [const { core::sync::atomic::AtomicU64::new(0) }; 24];
            static SCAN_J: [core::sync::atomic::AtomicU32; 24] =
                [const { core::sync::atomic::AtomicU32::new(0) }; 24];
            static SCAN_BURST: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            static SCAN_USED: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            let burst = (now / 500) as u32;
            let cur_burst = SCAN_BURST.load(Relaxed);
            if cur_burst != burst
                && SCAN_BURST
                    .compare_exchange(cur_burst, burst, Relaxed, Relaxed)
                    .is_ok()
            {
                SCAN_USED.store(0, Relaxed);
            }
            let used = SCAN_USED.load(Relaxed);
            let mut slot_due = false;
            let mut slot_ix = usize::MAX;
            for i in 0..24 {
                let t = SCAN_TGID[i].load(Relaxed);
                let w = SCAN_WORD[i].load(Relaxed);
                if t == g.tgid && w == g.uaddr as u64 {
                    slot_ix = i;
                    let j = SCAN_J[i].load(Relaxed);
                    slot_due = now.wrapping_sub(j as u64) >= 600;
                    break;
                }
                if t == 0 && slot_ix == usize::MAX {
                    slot_ix = i;
                    slot_due = true;
                }
            }
            if slot_due
                && used < 8
                && slot_ix != usize::MAX
                && SCAN_USED
                    .compare_exchange(used, used + 1, Relaxed, Relaxed)
                    .is_ok()
            {
                SCAN_TGID[slot_ix].store(g.tgid, Relaxed);
                SCAN_WORD[slot_ix].store(g.uaddr as u64, Relaxed);
                SCAN_J[slot_ix].store(now as u32, Relaxed);
                crate::api::sync::rwfrz_reader_scan(g.tgid, g.uaddr as u64);
                // 120th ROOT-CAUSE CORRECTION: the 118th's gate
                // `uaddr & 0xffff_f000 == 0x3fbf_13e000` used a 32-bit
                // mask against a 42-bit constant — always false, so LLVM
                // (correctly) DCE'd malloc_holder_scan out of every
                // release binary since the 118th. 64-bit page mask.
                let gu = g.uaddr as u64;
                if gu & 0xffff_ffff_f000 == 0x3fbf_13e000 {
                    crate::api::sync::malloc_holder_scan(g.tgid, gu);
                }
                // 120th GENERALIZED TRIGGER: a word that stood parked
                // >= 60 s with zero wakes gets the CoTask decoder with
                // THIS word as the key. The holder datum (a suspended
                // coroutine whose saved stack references the word) and
                // the resumer chains (owning serial queue active flag +
                // qname; WE ring; WUE deadline) are word-generic — only
                // the ALLOC-RIP/aframes arms are malloc-word-specific
                // and they add signal, never gate. The historical
                // foundation freeze keyed the malloc word; the
                // deterministic pre-foundation wedge (ctf15+ faces)
                // keys FFRT mutex/rwlock words in samgr/storage/param
                // instead — same release-side-never-runs family, and
                // now decodable. Cost bound: the per-tgid DONE latch
                // inside ctd_decode (16 slots) allows one decode per
                // tgid per boot.
                crate::api::sync::ctd_decode(g.tgid, gu, "W");
            }
        }
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
            // HOTPATH-PRINT: FUX-CHAIN — same rux:fuxstuck (retired debug=fuxstuck) gate and
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
                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                crate::pr_warn!(
                    "FUX-CHAIN-HEAD tid={} head={:#x} win:{}",
                    g.rep_tid,
                    g.rep_chn_head,
                    win_s
                );
            }
        }
        // 85th: park-page structure signature — page-start words and the
        // wait-word neighborhood; same rux:fuxstuck (retired debug=fuxstuck) gate + burst budget
        // as the row above (follows its group).
        if g.rep_pw0_ok || g.rep_pwu_ok {
            let mut w_s = alloc::string::String::new();
            if g.rep_pw0_ok {
                let _ = write!(w_s, " pw0:");
                for &wv in g.rep_pw0.iter() {
                    let _ = write!(w_s, " {:#x}", wv);
                }
            }
            if g.rep_pwu_ok {
                let _ = write!(w_s, " pwu[-28..+18]:");
                for &wv in g.rep_pwu.iter() {
                    let _ = write!(w_s, " {:#x}", wv);
                }
            }
            // HOTPATH-PRINT: park-page words — one line per reported
            // group, inside the same 8-line burst budget.
            crate::pr_warn!(
                "FUX-PAGE tid={} page={:#x}{}",
                g.rep_tid,
                g.uaddr & !0xfff,
                w_s
            );
        }
        fxrs_word_add(g.uaddr);
        printed += 1;
        if LINES.fetch_add(1, Relaxed) > 1200 {
            break;
        }
    }
    if total > printed && printed == 8 {
        // HOTPATH-PRINT: census overflow marker — same rux:fuxstuck (retired debug=fuxstuck) gate
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
                        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
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
                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
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
                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
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
                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                    crate::pr_warn!("{}", hdr);
                    for e in in_win.iter().take(12) {
                        if CTX_LINES.fetch_add(1, Relaxed) > 420 {
                            break;
                        }
                        // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
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
            crate::sched::hotpath::for_each_task_try(|task| {
                // SAFETY: diagnostic reads under the for_each_task
                // contract.
                let tk = crate::sched::r#unsafe::task_shared(task);
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
                crate::sched::hotpath::for_each_task_try(|task| {
                    // SAFETY: diagnostic reads under the for_each_task
                    // contract.
                    let tk = crate::sched::r#unsafe::task_shared(task);
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
                    } else if tk.ext.futex_wait_enqueued.load(Relaxed) {
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
                // HOTPATH-PRINT: fuxstuck graph row — same rux:fuxstuck (retired debug=fuxstuck)
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
                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
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
                        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
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
                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
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
        crate::timer::get_jiffies(),
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

    let current = match crate::sched::hotpath::current() {
        Some(c) => c as *mut Task,
        None => return,
    };
    let target = crate::timer::get_jiffies() + 1;
    let pid = crate::sched::r#unsafe::task_shared(current).pid();
    // arch-s5: typed one-shot guard — the scope-exit Drop cancels the
    // registration (the old explicit trailing del_timer).
    let Some(wake) = crate::timer::WakeArmed::arm_jiffy(target, pid) else {
        return; // no timer slot: busy retry (rare, bounded by wakers)
    };
    loop {
        if crate::timer::get_jiffies() >= target {
            break;
        }
        // Mark INTERRUPTIBLE BEFORE the final re-check (state-first
        // lost-wakeup discipline, same as sys_nanosleep).
        // SAFETY: current is the running task's pointer.
        crate::sched::r#unsafe::task_shared(current).set_state(TaskState::new(TaskState::INTERRUPTIBLE));
        if crate::timer::get_jiffies() >= target {
            // SAFETY: current is the running task's pointer.
            crate::sched::r#unsafe::task_shared(current).set_state(TaskState::new(TaskState::RUNNING));
            // A racing wake may have enqueued us while still executing —
            // take ourselves back off (NEW-C2 discipline).
            // SAFETY: current is the running task's pointer.
            crate::sched::hotpath::dequeue_task(crate::sched::r#unsafe::task_shared(current));
            break;
        }
        // SAFETY: schedule() operates on the current task.
        crate::sched::hotpath::schedule();
    }
    drop(wake);
}

/// Calculate futex hash value
pub fn futex_hash(key: &FutexKey) -> usize {
    // FWK (reference get_futex_key parity): object-keyed (SHARED-VMA)
    // words hash by the PHYSICAL word identity — a parent waiting on a
    // MAP_SHARED page at vaddr A and an exec'd child waking from a fresh
    // mmap at vaddr B share a bucket (the LTP tst_checkpoint protocol;
    // waitid08). Every anon word — PRIVATE-flag op or SHARED-semantic op
    // on a PRIVATE VMA — hashes (mm, uaddr): the musl cleartid interop
    // (priv=0 waiters vs the kernel's CLONE_CHILD_CLEARTID exit wake on
    // __thread_list_lock) lands both sides in ONE bucket by
    // construction, replacing the old phys-for-all bucketing. That old
    // scheme made anon keys frame-DEPENDENT: fork re-marks the page
    // RO+COW, the next store swaps the frame, and a parked waiter keyed
    // on the old frame became unreachable to every later wake — the
    // lostwake.c group strand (same-mm wake-miss with phys differing per
    // kill cycle; 8/8 parkers lost with a live waker).
    //
    // F12: the raw `uaddr % HASH_SIZE` clustered EVERY thread-stack
    // futex into ONE bucket. musl hands out 1 MB pthread stacks with a
    // fixed stride (0x101000 — stack + guard page), which is a multiple
    // of 64, so the ctid/join futex words of all threads of a process
    // landed in the same bucket: pth_str02's 1000-thread join chain
    // serialised on a single spinlock (all 256 pool slots chained in one
    // bucket, every wake/alloc walking 256 entries under that lock).
    // Fibonacci-multiply the identity so any fixed stride spreads
    // across the buckets.
    let x = if key.object {
        (key.phys >> 2) as u64
    } else {
        // mm: Arc allocation address (≥8-aligned); uaddr: 4-aligned.
        // Rotate the mm so its low entropy bits spread before the
        // Fibonacci multiply.
        (((key.mm as u64) >> 3).rotate_left(21)) ^ ((key.uaddr as u64) >> 2)
    };
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
    #[cfg(feature = "debug-futex-trace")]
    ftx_trace(b"WAKE-IN", uaddr, mm as u64, nr_wake as u64);

    let mut ret = 0i64;
    let mut prev_idx: Option<usize> = None;
    // Collect tasks to wake after releasing the bucket lock, like
    // kernel/futex/waitwake.c wake_futex() + wake_q_add(). Dynamically
    // sized: a fixed 8-entry array silently stranded every waiter past the
    // 8th (pthread_cond_broadcast) — review IPC-C3.
    //
    // NO-ALLOC-UNDER-LOCK (oh-launch round 4): the pushes below run
    // INSIDE the bucket-lock section, and a Vec growth there allocates
    // with IRQs off — the pile-up ingredient (fdk3: 3 CPUs spun 2 s+ on
    // HASH_HEADS+0x870 at 6-8 s). The reference's bucket sections never
    // allocate (wake_q is stack-preallocated; waiters embed their list
    // nodes). The walk breaks at ret >= nr_wake, so reserving nr_wake
    // slots BEFORE the lock makes the section allocation-free by
    // construction; nr_wake <= 0 callers never reach the walk.
    let mut wake_list: alloc::vec::Vec<*mut Task> =
        // CLAMP (fc1/fc2 postmortem): libc's pthread_cond_broadcast
        // passes nr_wake=INT_MAX — reserving it verbatim asked bigalloc
        // for 0x3fffffff8 bytes, the reject threw, and the
        // alloc_error_handler SIGKILLed init at 1.8 s (2/2 boots).
        // Real wakes on one bucket are single digits; broadcasts spike
        // to tens. 1024 covers every observed shape; an overflow past
        // it merely grows the Vec (the pre-fix behavior), not a die.
        alloc::vec::Vec::with_capacity((nr_wake as usize).clamp(1, 1024));

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
                        // Read-only pid of a waiter still linked in this
                        // bucket under our lock (INV-SCE-2).
                        // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
                        crate::pr_warn!(
                            "FUTEX-WAKE-THEFT waker mm={:#x} consumed waiter mm={:#x} uaddr={:#x} waiter tid {}",
                            mm,
                            waiter_mm,
                            uaddr,
                            crate::sched::r#unsafe::task_shared(woken_task).pid(),
                        );
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
    // timer-gap lane: every WAKE's walk outcome (rt2xdbg=1, 128 lines) —
    // uaddr, requested nr, matched/consumed ret, and the bucket chain
    // length at walk time. The pi_min wedge: did the gow WAKE even SEE
    // the parked waiters?
    if crate::cmdline::get_param("rt2xdbg").as_deref() == Some("1") {
        static FWW: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        if FWW.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 128 {
            use crate::debug::taskdump::putc_brief as putchar;
            fn ph(mut v: u64) {
                for _ in 0..16 {
                    let n = (v >> 60) as u8;
                    putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                    v <<= 4;
                }
            }
            let mut chain = 0u64;
            let mut idx = head.head;
            while let Some(i) = idx {
                chain += 1;
                if chain > 64 {
                    break;
                }
                let s = WAITER_POOL[i].lock_irqsave();
                idx = s.as_ref().map(|w| w.next).flatten();
            }
            for &b in b"FWW uaddr=0x" { putchar(b); }
            ph(uaddr as u64);
            for &b in b" nr=" { putchar(b); }
            ph(nr_wake as u64);
            for &b in b" ret=" { putchar(b); }
            ph(ret as u64);
            for &b in b" chain=" { putchar(b); }
            ph(chain);
            putchar(b'\n');
        }
    }

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
                    let tk = crate::sched::r#unsafe::task_shared(task);
                    // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
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
        && crate::trace::EVENT_FUXSTUCK.armed()
    {
        static MISS0_N: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        static MISS0_DUP: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let n = MISS0_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        if n < 240 {
            let waker = crate::sched::hotpath::current().map(|t| {
                (t.pid(), t.tgid() as u32)
            });
            // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
            crate::pr_warn!("FUXW-MISS0 #{} uaddr={:#x} waker={:?}", n, uaddr, waker);
        } else {
            MISS0_DUP.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
    }

    #[cfg(feature = "debug-futex-trace")]
    ftx_trace(b"WAKE-OUT", uaddr, ret as u64, woken_n as u64);

    // b133 hunt (rux:syscalltrace (retired debug=syscalltrace)): OH's samgr/SA threads park forever in
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
        && crate::trace::EVENT_FUTEX_MISS.armed()
    {
        static MISS_REPORTS: core::sync::atomic::AtomicU32 =
            core::sync::atomic::AtomicU32::new(0);
        let mut waker_pid = 0u32;
        if let Some(t) = crate::sched::hotpath::current() {
            // SAFETY: sched::current() yields the current task for the
            // duration of this syscall.
            waker_pid = crate::sched::r#unsafe::task_shared(t).pid();
        }
        for (bi, b) in HASH_HEADS.iter().enumerate() {
            let mut h = b.lock_irqsave();
            let mut idx = h.head;
            while let Some(i) = idx {
                // SAFETY: the slot index came from a chain link under the
                // bucket lock; slots are freed only under both locks.
                let (task, next, mismatch, w_phys) = {
                    let slot = WAITER_POOL[i].lock_irqsave();
                    match *slot {
                        Some(ref w) => {
                            let mismatch = w.key.uaddr == uaddr
                                && (!w.key.matches(&key) || (w.bitset & bitset) == 0);
                            (w.task, w.next, mismatch, w.key.phys)
                        }
                        None => (core::ptr::null_mut(), None, false, 0),
                    }
                };
                if mismatch {
                    let n = MISS_REPORTS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                    if n < 32 {
                        let mut wcomm = [0u8; 16];
                        if !task.is_null() {
                            // SAFETY: the task is linked in the bucket; it
                            // cannot be freed while we hold the lock.
                            let c = crate::sched::r#unsafe::task_shared(task).comm();
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
                        // FWK (futex-lostwake lane): phys on BOTH sides — a
                        // COW frame swap between the waiter's park and this
                        // wake shows waiter-phys != waker-phys with the same
                        // uaddr: the phys-keyed bucket moved and the wake
                        // scanned the waker's bucket only.
                        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                        crate::pr_warn!(
                            "futex-wake-miss: uaddr={:#x} waker pid {} key(mm={:#x},shared={},phys={:#x}) found waiter pid-key mm={:#x} phys={:#x} bucket {} waiter-comm {}",
                            uaddr,
                            waker_pid,
                            mm,
                            flags & FLAGS_SHARED != 0,
                            key.phys,
                            wmm,
                            w_phys,
                            bi,
                            name
                        );
                        // debug-migrate: the hit also records into the
                        // unified framework (rux:futex_miss carries the
                        // two mm identities — the lost-wake signature).
                        crate::trace::trace_futex_miss(uaddr as u64, mm as u64, wmm as u64, 1);
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

    let current = match crate::sched::hotpath::current() {
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
        let uval = match crate::sync::r#unsafe::word_read(uaddr_ptr as usize) {
            Some(v) => v,
            None => return -EFAULT as i64,
        };
        #[cfg(feature = "debug-futex-trace")]
        ftx_trace(b"WAIT-VAL", uaddr, uval as u64, val as u64);
        if uval != val {
            return -EAGAIN as i64;
        }

        // Deadline already passed while we were re-checking?
        // 63rd: deadlines are absolute monotonic ns now (was: jiffies).
        if let Some(dl) = deadline {
            if crate::timer::monotonic_ns_since_boot() >= dl {
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
                w.park_j = crate::timer::get_jiffies();
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
        crate::sched::r#unsafe::task_shared(current).ext.futex_wait_enqueued.store(true, core::sync::atomic::Ordering::Release);

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
                    // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
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
        {
            // INV-SCE-2: current is the parking task itself; the bucket
            // lock holds the chain stable (see the comment above).
            let current_t = crate::sched::r#unsafe::task_shared(current);
            // timer-gap lane: the park edge for cpu0-pinned FIFO tasks
            // (rt2xdbg=1, 64 lines) — the pi_min wedge question is
            // whether the waiter ever parks on the word the waker walks.
            if crate::cmdline::get_param("rt2xdbg").as_deref() == Some("1")
                && current_t.cpus_allowed() == 1
            {
                static FWT: core::sync::atomic::AtomicU32 =
                    core::sync::atomic::AtomicU32::new(0);
                if FWT.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 64 {
                    use crate::debug::taskdump::putc_brief as putchar;
                    fn ph(mut v: u64) {
                        for _ in 0..16 {
                            let n = (v >> 60) as u8;
                            putchar(if n < 10 { b'0' + n } else { b'a' + n - 10 });
                            v <<= 4;
                        }
                    }
                    for &b in b"FWT-PARK pid=0x" { putchar(b); }
                    ph(current_t.pid() as u64);
                    for &b in b" uaddr=0x" { putchar(b); }
                    ph(uaddr as u64);
                    for &b in b" val=0x" { putchar(b); }
                    ph(uval as u64);
                    putchar(b'\n');
                }
            }
            current_t.set_state(TaskState::new(TaskState::INTERRUPTIBLE));
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
                crate::syscall::support::jiffies_covering_deadline(dl),
                crate::syscall::support::deadline_ns_to_hres_ticks(dl),
                crate::sched::hotpath::get_current_pid(),
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
        crate::sched::hotpath::schedule();
        if let Some(id) = wake {
            if id != 0 {
                crate::timer::del_timer(id);
            }
        }
        #[cfg(feature = "debug-futex-trace")]
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
                    return -crate::errno::constants::EINTR as i64;
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
                        if crate::timer::monotonic_ns_since_boot() >= dl {
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
                    crate::sched::r#unsafe::task_shared(current).ext.futex_wait_enqueued.store(false, core::sync::atomic::Ordering::Release);
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
                        crate::sched::r#unsafe::task_shared(w.task).ext.futex_wait_enqueued.store(false, core::sync::atomic::Ordering::Release);
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
                            crate::sched::r#unsafe::task_shared(w.task).ext.futex_wait_enqueued.store(false, core::sync::atomic::Ordering::Release);
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
            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
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
    if !crate::sched::r#unsafe::task_shared(task).ext.futex_wait_enqueued.load(core::sync::atomic::Ordering::Acquire) {
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
    crate::sched::r#unsafe::task_shared(task).ext.futex_wait_enqueued.store(false, core::sync::atomic::Ordering::Release);
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
    let uncopied =
        crate::sync::r#unsafe::copy_from_user_bytes(buf.as_mut_ptr(), timeout_ptr as usize, 16);
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
            crate::timer::monotonic_ns_since_boot().saturating_add(spec_ns),
        ));
    }
    if clock_realtime {
        // WAIT_BITSET + FUTEX_CLOCK_REALTIME: absolute against the
        // realtime clock — rebase onto the monotonic timebase the wake
        // timers use. A deadline already in the past saturates to 0 and
        // expires on the next check (Linux returns ETIMEDOUT promptly).
        let wall = crate::timer::wall_epoch_offset_ns() as u128;
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
            match crate::sync::r#unsafe::word_read(uaddr) {
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
    //
    // NO-ALLOC-UNDER-LOCK (oh-launch round 4): pushes run inside the
    // (double) bucket-lock sections and are bounded by the walk's
    // woken < nr_wake break — reserve that capacity BEFORE the locks so
    // the sections never allocate (the reference's wake_q discipline;
    // see futex_wake_in_mm's note for the pile-up this closes).
    let mut wake_list: alloc::vec::Vec<*mut Task> =
        // CLAMP (fc1/fc2 postmortem): libc's pthread_cond_broadcast
        // passes nr_wake=INT_MAX — reserving it verbatim asked bigalloc
        // for 0x3fffffff8 bytes, the reject threw, and the
        // alloc_error_handler SIGKILLed init at 1.8 s (2/2 boots).
        // Real wakes on one bucket are single digits; broadcasts spike
        // to tens. 1024 covers every observed shape; an overflow past
        // it merely grows the Vec (the pre-fix behavior), not a die.
        alloc::vec::Vec::with_capacity((nr_wake as usize).clamp(1, 1024));

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
            match crate::sync::r#unsafe::word_read(uaddr) {
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
                let pid = crate::sched::r#unsafe::task_shared(*task).pid();
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
            match crate::sync::r#unsafe::word_read(uaddr) {
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
                let pid = crate::sched::r#unsafe::task_shared(*task).pid();
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
        let old = match crate::sync::r#unsafe::word_read(uaddr2) {
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
        if !crate::sync::r#unsafe::word_write(uaddr2 as usize, new) {
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

/// DEBUG: dump the live state of a futex word + its waiter chain.
#[cfg(feature = "debug-futex-trace")]
pub fn debug_dump_futex_state(uaddr: usize, parked_pid: u32) {
    use crate::debug::taskdump::{taskdump_dec, taskdump_raw_line};
    use crate::debug::taskdump::dump_syscall_ring_for;
    let bucket = uaddr % HASH_SIZE;
    taskdump_raw_line(b"FTX-STATE u=");
    taskdump_dec(uaddr as u64);
    // The user word and its neighbours (16 words starting uaddr-16).
    for i in 0..4 {
        let a = uaddr + 16 * i;
        match crate::sync::r#unsafe::word_read(a) {
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
            taskdump_dec(crate::sched::r#unsafe::task_shared(t).pid() as u64);
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
        // rwlock-livelock: latch the exit-watch tgid at the FIRST
        // plain-mutex contention WAIT (expected val 1/2 — glibc
        // pthread_mutex slow path) from a process whose tgid LEADER is
        // udisksd. The wedge's mutex holder matches no live task, so
        // the discriminating datum is whether a thread of this process
        // EXITS in the window before the waiter parks; the census adopts
        // the frozen word only minutes later, so the latch must ride
        // the very first contention.
        //
        // RETIRED (probe-retire): the latch self-armed on EVERY boot
        // (udisksd contention is ordinary on 24.04) and the exit.rs
        // forensics then printed per thread exit. Behind the existing
        // futex-hunt switch (rux:futextrace (retired debug=futextrace)), same as freeze_scan.
        if matches!(cmd, FUTEX_WAIT | FUTEX_WAIT_BITSET)
            && (val == 1 || val == 2)
            && RWFRZ_EXIT_WATCH_TGID.load(Relaxed) == 0
            && crate::trace::EVENT_FUTEX_MISS.armed()
        {
            if let Some(t) = crate::sched::hotpath::current() {
                // SAFETY: read-only tgid of the current task.
                let tgid = crate::sched::r#unsafe::task_shared(t).tgid() as u32;
                // SAFETY: pid-hash lookup of the tgid leader; read-only
                // comm read.
                let lead = crate::sched::hotpath::find_task_by_pid(tgid);
                if !lead.is_null()
                    && crate::sched::r#unsafe::task_shared(lead).comm().starts_with(b"udisksd")
                {
                    let _ = RWFRZ_EXIT_WATCH_TGID.compare_exchange(
                        0,
                        tgid,
                        Relaxed,
                        Relaxed,
                    );
                }
            }
        }
        // 120th (cotask-decode): the malloc-lock wedge EARLY trigger.
        // ld-musl's __malloc_lock sits at the fixed VA 0x3fbf13e6a8 in
        // every OH process; the historical foundation freeze latches it
        // held-forever with waiters arriving 1:1 onto the low count.
        // Healthy malloc contention turns over in <100 ms (EAGAIN churn),
        // so >=2 s of CONTINUOUS held-bit WAITs from one tgid is the
        // wedge signature — and the waiters' own FUTEX_WAIT is the first
        // kernel-visible event of it (the census adopts the word only
        // ~60 s later; the OH68 acquire latch is process-blind). On the
        // 2 s persistence edge: arm the write watch (a late unlock then
        // logs an FW row with rip), run the thread-side holder hunt, and
        // run the CoTask saved-stack decoder — while any live holder is
        // still parked mid-critical-section and nameable. Once per tgid;
        // rux:fuxstuck (retired debug=fuxstuck)-gated (the page compare below is the only cost
        // on switch-off boots).
        // 120th ROOT-CAUSE CORRECTION (presence hunt): the 118th's
        // original gate compared `uaddr & 0xffff_f000 == 0x3fbf_13e000`
        // — but 0xffff_f000 is a 32-bit mask, so the masked value can
        // never carry the constant's bits 32..41: the compare was
        // ALWAYS false and LLVM (correctly) DCE'd the whole subtree out
        // of every release build, including the 120th atomic+black_box
        // variant which kept the broken mask. The page mask must be
        // 64-bit; the plain literal compare is runtime-variable and
        // survives optimization (verified on a rustc -O repro).
        const CTDE_PAGE: u64 = 0x3fbf_13e000;
        const CTDE_WORD: u64 = 0x3fbf_13e6a8;
        let ctde_uaddr = uaddr as u64;
        if matches!(cmd, FUTEX_WAIT | FUTEX_WAIT_BITSET)
            && ctde_uaddr & 0xffff_ffff_f000 == CTDE_PAGE
            && val & 0x8000_0000 != 0
            && val & 0x7fff_ffff >= 2
            && crate::trace::EVENT_FUXSTUCK.armed()
        {
            const CTDE_SLOTS: usize = 8;
            static CTDE_TGID: [core::sync::atomic::AtomicU32; CTDE_SLOTS] =
                [const { core::sync::atomic::AtomicU32::new(0) }; CTDE_SLOTS];
            static CTDE_J: [core::sync::atomic::AtomicU32; CTDE_SLOTS] =
                [const { core::sync::atomic::AtomicU32::new(0) }; CTDE_SLOTS];
            static CTDE_FIRED: [core::sync::atomic::AtomicBool; CTDE_SLOTS] =
                [const { core::sync::atomic::AtomicBool::new(false) }; CTDE_SLOTS];
            if let Some(t) = crate::sched::hotpath::current() {
                // SAFETY: read-only pid/tgid of the current task.
                let (tid, tgid) = { (t.pid() as u32, t.tgid() as u32) };
                let now = crate::timer::get_jiffies() as u32;
                let mut slot = usize::MAX;
                let mut fire = false;
                for i in 0..CTDE_SLOTS {
                    let s = CTDE_TGID[i].load(Relaxed);
                    if s == tgid {
                        slot = i;
                        if !CTDE_FIRED[i].load(Relaxed)
                            && now.wrapping_sub(CTDE_J[i].load(Relaxed)) >= 200
                        {
                            fire = CTDE_FIRED[i]
                                .compare_exchange(false, true, Relaxed, Relaxed)
                                .is_ok();
                        }
                        break;
                    }
                    if s == 0 && slot == usize::MAX {
                        slot = i;
                        // claim the slot with its start jiffies; the
                        // persistence check runs on later waits.
                        if CTDE_TGID[i]
                            .compare_exchange(0, tgid, Relaxed, Relaxed)
                            .is_ok()
                        {
                            CTDE_J[i].store(now, Relaxed);
                        }
                    }
                }
                if fire {
                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                    crate::pr_warn!(
                        "CTD-EARLY tgid={} tid={} word=0x3fbf13e6a8 val={:#x} (held >=2s — full decode now)",
                        tgid,
                        tid,
                        val
                    );
                    let ctw = CTDE_WORD;
                    crate::api::sync::fuxpark_latch(ctw, tid, tgid);
                    crate::api::sync::malloc_holder_scan(tgid, ctw);
                    crate::api::sync::ctd_decode(tgid, ctw, "E");
                }
            }
        }
        // rwlock-livelock: the guard read only slots 0/1 — a word latched
        // into a later slot (glibc-shape latch after musl parks took the
        // front slots) never printed. Scan all 8; the all-zero fast exit
        // is the same cost as the old two loads.
        for w in RWFRZ_WATCH.iter() {
            let wa = w.load(Relaxed);
            if wa != 0 && wa == uaddr {
                let c = RWFRZ_WATCH_CNT.fetch_add(1, Relaxed);
                if c < RWFRZ_WATCH_BUDGET.load(core::sync::atomic::Ordering::Relaxed) {
                    let (tid, tgid) = match crate::sched::hotpath::current() {
                        Some(t) => { (t.pid(), t.tgid()) },
                        None => (0, 0),
                    };
                    // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
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
                        crate::timer::get_jiffies(),
                    );
                }
                break;
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
    // polled-16550 time). The hunt is gated behind rux:futextrace (retired debug=futextrace) (the
    // same switch as the wake-miss scan in futex_wake_in_mm); the
    // sc184/sc185 hunts re-arm with the flag.
    if crate::trace::EVENT_FUTEX_MISS.armed()
    {
        use core::sync::atomic::Ordering::Relaxed;
        let cur_tgid =
            crate::sched::hotpath::current().map(|t| crate::sched::r#unsafe::task_shared(t).tgid() as u32);
        // sc185: the SA-5100 parcel latch never fired in sc184 (zero FXDV
        // lines), so also latch by process name — the first futex from any
        // thread of "hdf_devmgr" latches its tgid. The parcel matcher stays
        // (earlier latch = earlier coverage), this is the backstop.
        if FXDV_PID.load(Relaxed) == 0 {
            if let Some(t) = crate::sched::hotpath::current() {
                let comm = crate::sched::r#unsafe::task_shared(t).comm();
                let name = core::str::from_utf8(&comm[..])
                    .unwrap_or("")
                    .trim_end_matches('\0');
                if name == "hdf_devmgr" {
                    crate::api::sync::fxdv_latch_devmgr(crate::sched::r#unsafe::task_shared(t).tgid() as u32);
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
                    crate::debug::taskdump::ensure_defer_thread();
                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                    crate::pr_warn!("FXDV-ARM slot1={:#x} jif={}", uaddr, crate::timer::get_jiffies());
                } else if (s1 as usize) != uaddr && FUXWATCH2_ADDR.load(Relaxed) == 0 {
                    FUXWATCH2_ADDR.store(uaddr as u64, Relaxed);
                    crate::debug::taskdump::ensure_defer_thread();
                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                    crate::pr_warn!("FXDV-ARM slot2={:#x} jif={}", uaddr, crate::timer::get_jiffies());
                }
                // 70th (ohw86): birth-curve sampler — on every traced devmgr
                // futex, sample slot1's word AND its +0x94 neighbor (the
                // mutex cluster: sem at mutex-0x94 in every observed boot;
                // ohw86 proved the frozen mutex took its last write BEFORE
                // any wait armed the page watch) and print each CHANGE.
                // The final 0->0x10 transition's timestamp brackets the
                // founder to the second, before any watch can arm.
                if let Some(t) = crate::sched::hotpath::current() {
                    let base = FUXWATCH_ADDR.load(Relaxed) as usize;
                    if base != 0 {
                        static LAST1: [core::sync::atomic::AtomicU32; 2] =
                            [const { core::sync::atomic::AtomicU32::new(u32::MAX) }; 2];
                        for (k, wa) in [base, base.wrapping_add(0x94)].iter().enumerate() {
                            if let Some(w) = crate::sync::r#unsafe::read_u32_via_task(t, *wa) {
                                let prev = LAST1[k].load(Relaxed);
                                if w != prev {
                                    LAST1[k].store(w, Relaxed);
                                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                                    crate::pr_warn!(
                                        "FXDV-CURVE word{:+#x} {:#x} -> {:#x} jif={} tid={}",
                                        if k == 0 { 0 } else { 0x94 },
                                        prev,
                                        w,
                                        crate::timer::get_jiffies(),
                                        crate::sched::r#unsafe::task_shared(t).pid()
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
                if let Some(t) = crate::sched::hotpath::current() {
                    if let Some(w) = crate::sync::r#unsafe::read_u32_via_task(t, uaddr) {
                        let c = if addr_idx == usize::MAX {
                            0
                        } else {
                            FXDV_CNT[addr_idx].load(Relaxed)
                        };
                        if c < 8 || c % 32 == 0 {
                            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                            crate::pr_warn!(
                                "FXDV-WORD tid={} uaddr={:#x} word={:#x} want={:#x} jif={}",
                                crate::sched::r#unsafe::task_shared(t).pid(),
                                uaddr,
                                w,
                                val,
                                crate::timer::get_jiffies()
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
                        // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
                        crate::pr_warn!(
                            "FXDV #{} tid={} op={} uaddr={:#x} val={:#x} jif={}",
                            n,
                            crate::sched::hotpath::current()
                                .map(|t| t.pid())
                                .unwrap_or(0),
                            match cmd {
                                FUTEX_WAIT => "WAIT",
                                FUTEX_WAIT_BITSET => "WAITB",
                                _ => "WAKE",
                            },
                            uaddr,
                            val,
                            crate::timer::get_jiffies(),
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
        // GATED behind rux:futextrace (retired debug=futextrace) since the lb243 decode: this tracer
        // ran ALWAYS-ON from inside do_futex's hot path, and the param
        // arena is the boot critical path (param_service sync + every
        // param read). Its 500 budgeted console writes (each allocating +
        // 0.36-0.93ms UART) all execute in the first seconds of boot; when
        // the still-unfixed phantom-preempt allocator stall bites
        // mid-print, the printing thread parks INSIDE do_futex with the
        // print half-done (lb243: samgr's self-send worker frozen at
        // do_futex+0x112e over LowerHex/unix_write_slice_owned residue)
        // and the param sync chain freezes the whole service graph — the
        // 'samgr-dark' boot family. Hunt with rux:futextrace (retired debug=futextrace), never by
        // default.
        if (0x7ff0_0000_0000..0x7ff0_0010_0000).contains(&uaddr)
            && crate::trace::EVENT_FUTEX_MISS.armed()
        {
            use core::sync::atomic::Ordering::Relaxed as R2;
            let n = FXDV2_N.fetch_add(1, R2);
            if n < 500 {
                // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
                crate::pr_warn!(
                    "FXDV2 #{} tgid={} tid={} op={} uaddr={:#x} val={:#x} jif={}",
                    n,
                    crate::sched::hotpath::current().map(|t| t.tgid()).unwrap_or(0),
                    crate::sched::hotpath::current().map(|t| t.pid()).unwrap_or(0),
                    match cmd {
                        FUTEX_WAIT => "WAIT",
                        FUTEX_WAIT_BITSET => "WAITB",
                        FUTEX_WAKE => "WAKE",
                        FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => "REQUEUE",
                        _ => "other",
                    },
                    uaddr,
                    val,
                    crate::timer::get_jiffies(),
                );
            }
        }
    }

    #[cfg(feature = "debug-futex-trace")]
    {
        let pid = crate::sched::hotpath::current().map(|t| crate::sched::r#unsafe::task_shared(t).pid() as u64).unwrap_or(0);
        use crate::debug::taskdump::{taskdump_dec, taskdump_raw_line};
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
            if crate::sync::r#unsafe::word_read(uaddr2).is_none() {
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
        FUTEX_LOCK_PI | FUTEX_LOCK_PI2 | FUTEX_TRYLOCK_PI | FUTEX_UNLOCK_PI => {
            // RT-2X item 7 (paired-test queue): dispatch the PI ops to
            // the long-implemented-but-never-wired futex_pi module (the
            // pair test suite's pi_stress row failed here with ENOSYS).
            // Shared/private rides the FLAGS_PRIVATE bit like every
            // other op. LOCK_PI/LOCK_PI2 treat the timeout as an
            // ABSOLUTE CLOCK_MONOTONIC timespec (the module parks on an
            // absolute-jiffy deadline); TRYLOCK/UNLOCK take none.
            let shared = (flags & FLAGS_SHARED) != 0;
            match cmd {
                FUTEX_TRYLOCK_PI => crate::sync::futex_pi::futex_trylock_pi(uaddr, shared),
                FUTEX_UNLOCK_PI => crate::sync::futex_pi::futex_unlock_pi(uaddr, shared),
                _ => {
                    let dl = match futex_parse_timeout_ns(_timeout, true, false) {
                        Ok(Some(ns)) => {
                            // ns (absolute, boot-anchored MONOTONIC) ->
                            // absolute jiffies, rounded up so an
                            // in-progress jiffy never under-times
                            let hz = crate::config::KERNEL_HZ as u64;
                            Some(ns.saturating_mul(hz).div_ceil(1_000_000_000))
                        }
                        Ok(None) => None,
                        Err(e) => return -(e as i64),
                    };
                    crate::sync::futex_pi::futex_lock_pi(uaddr, dl, shared)
                }
            }
        }
        _ => {
            // The remaining PI family (WAIT_REQUEUE_PI / CMP_REQUEUE_PI,
            // the glibc PI-condvar path) is not yet implemented.
            -ENOSYS as i64
        }
    };
    // rwlock-livelock: post-dispatch twin of the RWFRZ-WATCH entry print.
    // The verdict ladder needs op OUTCOMES on the frozen word, not just
    // arrivals: a WAKE that returned 0 while a waiter stays parked is the
    // (b) wake-miss shape; a WAKE that returned >=1 with the waiter still
    // parked is (c) re-park; a WAIT returning -EAGAIN in a loop is the
    // userspace spin itself; total silence after the latch is (a)
    // never-sent. glibc's rwlock retry ladder keys off futex_wake's
    // return count, so ret is printed for every op class. Shares the
    // 192-line budget with the entry print (a spinner eats it in ms, but
    // the first 192 lines ARE the loop's shape).
    {
        use core::sync::atomic::Ordering::Relaxed;
        for w in RWFRZ_WATCH.iter() {
            let wa = w.load(Relaxed);
            if wa != 0 && wa == uaddr {
                let c = RWFRZ_WATCH_CNT.fetch_add(1, Relaxed);
                if c < RWFRZ_WATCH_BUDGET.load(core::sync::atomic::Ordering::Relaxed) {
                    let (tid, tgid) = match crate::sched::hotpath::current() {
                        Some(t) => { (t.pid(), t.tgid()) },
                        None => (0, 0),
                    };
                    // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
                    crate::pr_warn!(
                        "RWFRZ-WATCH-RET #{} tid={} tgid={} op={} uaddr={:#x} val={:#x} ret={} jif={}",
                        c,
                        tid,
                        tgid,
                        match cmd {
                            FUTEX_WAIT => "WAIT",
                            FUTEX_WAIT_BITSET => "WAITB",
                            FUTEX_WAKE => "WAKE",
                            FUTEX_WAKE_BITSET => "WAKEB",
                            FUTEX_WAKE_OP => "WAKEOP",
                            FUTEX_REQUEUE | FUTEX_CMP_REQUEUE => "REQUEUE",
                            _ => "other",
                        },
                        uaddr,
                        val,
                        ret,
                        crate::timer::get_jiffies(),
                    );
                }
                break;
            }
        }
    }
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
    // 76th (launcher lane, rux:txnhunt (retired debug=txnhunt)): samgr FFRT wake audit. The
    // accountmgr ready-flag gate (ohw106): accountmgr's four SA-listener
    // subscribes post two subscribe-time OnAddSystemAbility notify tasks
    // into samgr's FFRT workHandler queue ~30 ms apart — the FIRST runs
    // (notify sent 26.375 s) and the SECOND never does, silently
    // dropping the BMS ready callback and with it the whole StartUser(46)
    // chain. FFRT dispatches queues via futex wakes on its worker pool;
    // this tap prints every WAKE-class op issued by a samgr thread
    // during the first 70 s so the missing PostTask's wake (and its ret
    // — whether a parked worker was actually found) is visible. Zero
    // cost when rux:txnhunt (retired debug=txnhunt) is off.
    if matches!(
        cmd,
        FUTEX_WAKE | FUTEX_WAKE_BITSET | FUTEX_WAKE_OP | FUTEX_REQUEUE | FUTEX_CMP_REQUEUE
    ) {
        let sp = crate::ipc::binder::binder_sareg_samgr_pid();
        if sp != 0 {
            let mine = crate::sched::hotpath::current()
                .map(|t| crate::sched::r#unsafe::task_shared(t).tgid() == sp)
                .unwrap_or(false);
            if mine
                && crate::trace::EVENT_TXNHANG.armed()
                && crate::timer::get_jiffies() <= 7000
            {
                static SMW_N: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                if SMW_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed) < 3000 {
                    let tid = crate::sched::hotpath::current()
                        .map(|t| t.pid())
                        .unwrap_or(0);
                    let comm = crate::sched::hotpath::current()
                        .map(|t| t.comm())
                        .map(|c| core::str::from_utf8(c).unwrap_or(""))
                        .unwrap_or("")
                        .trim_end_matches('\0');
                    // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
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
    // 118th (mouse-endgame): the cap is now PER-TGID 3 within a global
    // 32 — the 16 global dumps all landed in the 2.6-3.4 s startup
    // contention burst (hilogd, samgr FFRT workers, init children —
    // every one transient), so the wedge-window park of foundation tid
    // 1332 on the 0x80000010 mutex at ~13.6 s (the exact caller-chain
    // this arm exists for) never dumped. Per-tgid budgeting keeps the
    // early spam bounded while leaving room for late parks.
    if matches!(op & FUTEX_CMD_MASK, FUTEX_WAIT | FUTEX_WAIT_BITSET) {
        let readers = val & 0x7fffffff;
        if val & 0x80000000 != 0 && readers >= 4 && readers < 0x40000000 {
            static PARKED_DUMPS: core::sync::atomic::AtomicU32 =
                core::sync::atomic::AtomicU32::new(0);
            static PARK_TGID: [core::sync::atomic::AtomicU32; 8] =
                [const { core::sync::atomic::AtomicU32::new(0) }; 8];
            static PARK_TGID_N: [core::sync::atomic::AtomicU32; 8] =
                [const { core::sync::atomic::AtomicU32::new(0) }; 8];
            let n = PARKED_DUMPS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            // Per-tgid accounting: find this tgid's slot (or the first
            // free one) and take one unit of its 3-dump budget.
            use core::sync::atomic::Ordering::Relaxed;
            let cur_tgid = crate::sched::hotpath::current()
                .map(|t| crate::sched::r#unsafe::task_shared(t).tgid() as u32)
                .unwrap_or(0);
            let mut tslot = usize::MAX;
            for i in 0..8 {
                let t = PARK_TGID[i].load(Relaxed);
                if cur_tgid != 0 && t == cur_tgid {
                    tslot = i;
                    break;
                }
                if t == 0 && tslot == usize::MAX {
                    tslot = i;
                }
            }
            let tgid_ok = tslot != usize::MAX && {
                // Claim a free slot for this tgid (racy claim is fine —
                // a lost claimant simply skips its dump this once).
                if PARK_TGID[tslot].load(Relaxed) == 0 {
                    let _ = PARK_TGID[tslot]
                        .compare_exchange(0, cur_tgid, Relaxed, Relaxed);
                }
                PARK_TGID[tslot].load(Relaxed) == cur_tgid
                    && PARK_TGID_N[tslot].fetch_add(1, Relaxed) < 3
            };
            // rux:parkdump (retired debug=parkdump): each park dump costs hundreds of UART+kmsg
            // lines (hilogd.rd_kmsg eats a vCPU copying them out of
            // /dev/kmsg — the b128/b129 lesson), so the park-dump family
            // is OFF in launcher-milestone boots.
            if n < 32
                && tgid_ok
                && crate::trace::EVENT_PARKDUMP.armed()
            {
                if let Some(t) = crate::sched::hotpath::current() {
                    let tk = crate::sched::r#unsafe::task_shared(t);
                    let comm = tk.comm();
                    let name = core::str::from_utf8(&comm[..])
                        .unwrap_or("")
                        .trim_end_matches('\0');
                    crate::trace::trace_parkdump(0, tk.pid(), uaddr as u64, val as u64);
                    // HOTPATH-PRINT: switch/cmdline-gated (off by default).
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
                    rwfz_stack_frames(t, "park", 12, 64, false);
                }
            }
        }
    }
    // sc122 (Session P): the app-child park dump. The KILLDUMP path can
    // never sample the park — SIGKILL lands right after the userspace DEBUG
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
            if let Some(t) = crate::sched::hotpath::current() {
                let tk = crate::sched::r#unsafe::task_shared(t);
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
                    // rux:parkdump (retired debug=parkdump): the APP-PARK dump prints the child's
                    // whole lib-region VMA table (APPMAP, ~2.9k lines per
                    // dump in sc179) + fd table + syscall rings — pure
                    // chain forensics, OFF in milestone boots (b128/b129
                    // hilogd.rd_kmsg lesson).
                    if n < 6
                        && crate::trace::EVENT_PARKDUMP.armed()
                    {
                        crate::trace::trace_parkdump(1, tk.pid(), uaddr as u64, val as u64);
                        // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
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
                            let cur = tk.ext.last_calls_cur.load(Relaxed);
                            let mut s2 = alloc::string::String::new();
                            use core::fmt::Write;
                            let _ = write!(s2, "APP-PARK calls:");
                            for k in 0..RL {
                                let i = (cur + RL - k) % RL; // newest first
                                let v = tk.ext.last_calls[i].load(Relaxed);
                                if v != 0 {
                                    let _ = write!(s2, " {}", v);
                                }
                            }
                            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
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
                            let cur = tk.ext.init_calls_cur.load(Relaxed);
                            // sc143: 512 entries in chunks of 128 so a
                            // single UART flush cannot gut the whole line.
                            let mut chunk = alloc::string::String::new();
                            let mut shown = 0usize;
                            for k in 0..RL {
                                let i = (cur + RL - k) % RL; // newest first
                                let v = tk.ext.init_calls[i].load(Relaxed);
                                if v != 0 {
                                    if shown % 128 == 0 {
                                        if !chunk.is_empty() {
                                            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
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
                                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                                crate::pr_warn!("{}", chunk);
                            }
                            // newest 96 with a0
                            let mut s3 = alloc::string::String::new();
                            let _ = write!(s3, "APP-PARK initA:");
                            for k in 0..96.min(RL) {
                                let i = (cur + RL - k) % RL;
                                let v = tk.ext.init_calls[i].load(Relaxed);
                                if v != 0 {
                                    let a0 = tk.ext.init_a0[i].load(Relaxed);
                                    let _ = write!(s3, " {}@{:#x}", v, a0);
                                }
                            }
                            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                            crate::pr_warn!("{}", s3);
                            // sc143: the birth window (fork-exit onward),
                            // immune to loop churn.
                            let mut s4 = alloc::string::String::new();
                            let _ = write!(s4, "APP-PARK birth:");
                            for k in 0..BL {
                                let v = tk.ext.birth_calls[k].load(Relaxed);
                                if v != 0 {
                                    let a0 = tk.ext.birth_a0[k].load(Relaxed);
                                    let _ = write!(s4, " {}@{:#x}", v, a0);
                                }
                            }
                            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                            crate::pr_warn!("{}", s4);
                        }
                        // sc125 (Session P): the pre-park FAILING-syscall
                        // tail — musl pthread_create mmaps the thread stack
                        // BEFORE clone, so an ENOMEM mmap means no clone
                        // ever reaches the census. Latched per task.
                        {
                            use core::sync::atomic::Ordering::Relaxed;
                            let cur = tk.ext.last_fails_cur.load(Relaxed);
                            let mut s = alloc::string::String::new();
                            use core::fmt::Write;
                            let _ = write!(s, "APP-PARK fails:");
                            for k in 0..8 {
                                let i = (cur + 8 - 1 - k) % 8; // newest first
                                let v = tk.ext.last_fails[i].load(Relaxed);
                                if v != 0 {
                                    // sc135 (Session Q): a0/a1 name the
                                    // prctl option (nr167) and friends.
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
                            // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                            crate::pr_warn!("{}", s);
                        }
                        // SAFETY: t is the current task, in its own syscall.
                        rwfz_stack_frames(t, "apppark", 12, 96, false);
                        // sc139: the call-site-validated caller chain.
                        apppark_rets(t);
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
                                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
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
                                            if crate::api::net::is_unix_socket_ops(ops) {
                                                match crate::api::net::unix_fd_bound_peer(
                                                    fd as usize,
                                                ) {
                                                    Some((bn, peer)) => {
                                                        alloc::format!(
                                                            "unix bound={} peer={}",
                                                            bn, peer
                                                        )
                                                    }
                                                    None => "unix".into(),
                                                }
                                            } else if crate::api::net::is_inet_socket_ops(
                                                ops,
                                            ) {
                                                "inet-socket".into()
                                            } else if crate::api::net::is_netlink_ops(ops) {
                                                "netlink".into()
                                            } else if crate::api::net::is_raw_ops(ops) {
                                                "raw-socket".into()
                                            } else if core::ptr::eq(
                                                p,
                                                &crate::ipc::binder::BINDER_OPS,
                                            ) {
                                                "binder".into()
                                            } else if crate::api::fs::is_pipe_ops(ops) {
                                                "pipe".into()
                                            } else if crate::api::fs::is_memfd_ops(ops) {
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
                                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
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
                                            {
                                                let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
                                                regs.rsp as usize
                                            }
                                        }
                                        #[cfg(feature = "riscv64")]
                                        {
                                            {
                                                let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
                                                regs.sp as usize
                                            }
                                        }
                                        #[cfg(not(any(feature = "x86_64", feature = "riscv64")))]
                                        { 0usize }
                                    }
                                    None => 0usize,
                                };
                                if rsp != 0 {
                                    let rd64 = |va: usize| -> Option<u64> {
                                        let lo = crate::sync::r#unsafe::read_u32_via_task(t, va)?;
                                        let hi = crate::sync::r#unsafe::read_u32_via_task(t, va + 4)?;
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
                                        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                                        crate::pr_warn!("{}", v);
                                    }
                                }
                            }
                            // (b) The global unix-socket registry once:
                            // which abstract/filesystem listeners exist at
                            // park time (the appconn hex was unreadable —
                            // strncpy stopped at the abstract NUL).
                            if n == 0 {
                                for line in crate::api::net::unix_registry_snapshot() {
                                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
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
    // rux:futextrace (retired debug=futextrace) family; the sc118/sc119 hunts re-arm with the
    // flag.
    {
        const STK_PAGE: usize = 0x7fffffffc000;
        if uaddr & !0xfff == STK_PAGE
            && crate::trace::EVENT_FUTEX_MISS.armed()
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
                let comm = crate::sched::hotpath::current()
                    .map(|t| t.comm())
                    .map(|c| core::str::from_utf8(c).unwrap_or(""))
                    .unwrap_or("")
                    .trim_end_matches('\0');
                let tid = crate::sched::hotpath::current()
                    .map(|t| t.pid())
                    .unwrap_or(0);
                // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
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
                // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
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
            let mine = crate::sched::hotpath::current()
                .map(|t| crate::sched::r#unsafe::task_shared(t).tgid() == lt)
                .unwrap_or(false);
            if mine || word_hit {
                let n = FXRS_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < FXRS_BUDGET {
                    let tid = crate::sched::hotpath::current()
                        .map(|t| t.pid())
                        .unwrap_or(0);
                    let tgid = crate::sched::hotpath::current()
                        .map(|t| t.tgid())
                        .unwrap_or(0);
                    let comm = crate::sched::hotpath::current()
                        .map(|t| t.comm())
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
    // pr_warn lines per OH boot. The census joins the rux:futextrace (retired debug=futextrace)
    // family; the sc156 hunt re-arms with the flag.
    if matches!(op & FUTEX_CMD_MASK, FUTEX_WAIT | FUTEX_WAIT_BITSET)
        && timeout != 0
        && crate::trace::EVENT_FUTEX_MISS.armed()
    {
        if let Some(t) = crate::sched::hotpath::current() {
            let tk = crate::sched::r#unsafe::task_shared(t);
            let comm = core::str::from_utf8(&tk.comm()[..])
                .unwrap_or("")
                .trim_end_matches('\0');
            if comm.contains("foundation") || comm.contains("sa_main") {
                static FTW_N: core::sync::atomic::AtomicUsize =
                    core::sync::atomic::AtomicUsize::new(0);
                let n = FTW_N.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                if n < 240 {
                    let mut ts = [0u8; 16];
                    let ts_ok = crate::sync::r#unsafe::copy_from_user_bytes(
                        ts.as_mut_ptr(),
                        timeout as usize,
                        ts.len(),
                    ) == 0;
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
                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
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
                        let cur = tk.ext.last_calls_cur.load(Relaxed);
                        let mut s2 = alloc::string::String::new();
                        let _ = write!(s2, "FTWAIT calls:");
                        for k in 0..RL {
                            let i = (cur + RL - k) % RL; // newest first
                            let v = tk.ext.last_calls[i].load(Relaxed);
                            if v != 0 {
                                let _ = write!(s2, " {}", v);
                            }
                        }
                        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                        crate::pr_warn!("{}", s2);
                        let fcur = tk.ext.last_fails_cur.load(Relaxed);
                        let mut s3 = alloc::string::String::new();
                        let _ = write!(s3, "FTWAIT fails:");
                        for k in 0..8 {
                            let i = (fcur + 8 - 1 - k) % 8;
                            let v = tk.ext.last_fails[i].load(Relaxed);
                            if v != 0 {
                                let a0 = tk.ext.last_fails_a0[i].load(Relaxed);
                                let _ = write!(s3, " nr{}=-{} a0={:#x}", v & 0xffff, v >> 16, a0);
                            }
                        }
                        // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
                        crate::pr_warn!("{}", s3);
                    }
                    let r = do_futex(uaddr, op, val, timeout, uaddr2, 0, val3);
                    rwfz_op_log(uaddr, op, val, r);
                    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
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
pub static RWFRZ_UADDR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);
pub static RWFRZ_OPLOG: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
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
    let tid = crate::sched::hotpath::current()
        .map(|t| t.pid() as u64)
        .unwrap_or(0);
    // HOTPATH-PRINT: budget-gated hunt diagnostic (first-N counter above).
    crate::pr_warn!(
        "RWFRZ-OP #{} {} tid {} val={:#x} ret={}",
        n,
        cmd,
        tid,
        val,
        result
    );
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
                if let Some(name) = crate::api::fs::memfd_name_by_ino(ino) {
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
/// Public shim for the exit-path forensics (process/exit.rs): walk the
/// DYING thread's user stack — its frames are intact at do_exit time.
/// SAFETY: caller guarantees `task` is the current, still-live task.
pub fn apppark_rets_public(task: *const Task) {
    apppark_rets(task)
}

/// IMMEDIATELY BEFORE it decode as a call instruction — `E8 rel32`
/// (v-5) or `FF D0-D7` call-reg (v-2) — the cheap x86-64 signature of
/// a real return address. Slots surviving that check are the real
/// frames. SAFETY: `task` live; diagnostic reads through its tables.
pub fn apppark_rets(task: *const Task) {
    let tk = crate::sched::r#unsafe::task_shared(task);
    // Stack-freed guard (KILLDUMP census race class): nothing to walk
    // without a live stack.
    let Some(top) = tk.kernel_stack_top_checked() else {
        return;
    };
    #[cfg(feature = "x86_64")]
    let rsp = {
        let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
        regs.rsp as usize
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
        let lo = crate::sync::r#unsafe::read_u32_via_task(task, va)?;
        let hi = crate::sync::r#unsafe::read_u32_via_task(task, va + 4)?;
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
    // HOTPATH-PRINT: hunt-era diagnostic on a rare adjudication path (wedge/freeze class).
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
pub fn rwfz_emit_prio(s: &str, prio: bool) {
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
            crate::debug::taskdump::defer_emit_line(alloc::format!(
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
pub fn rwfz_stack_frames(
    task: *const Task,
    label: &str,
    max_frames: u64,
    scan_slots: u64,
    prio: bool,
) {
    let tk = crate::sched::r#unsafe::task_shared(task);
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
        let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
        // SAFETY: diagnostic read of this task's entry frame.
        { (regs.rsp, regs.rip) }
    };
    #[cfg(feature = "riscv64")]
    let (rsp, rip) = {
        let regs = crate::sched::r#unsafe::pt_regs_at_top(top);
        // SAFETY: same contract, riscv64.
        { (regs.sp, regs.epc) }
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
        let lo = crate::sync::r#unsafe::read_u32_via_task(task, va as usize)?;
        let hi = crate::sync::r#unsafe::read_u32_via_task(task, (va + 4) as usize)?;
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
