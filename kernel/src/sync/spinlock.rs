//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Spinlock with preempt / IRQ / BH variants.
//!
//! API:
//!   lock()          — preempt disable + lock
//!   lock_irq()      — disable interrupts + preempt disable + lock
//!   lock_irqsave()  — save interrupt state + disable + preempt disable + lock
//!   lock_bh()       — disable bottom-half (softirq) + lock
//!
//! Backend: TTAS (test-and-test-and-set) via compare_exchange.
//! Ticket lock causes interactive-input deadlock on QEMU
//! (likely QEMU's amoadd.w emulation bug), so TTAS is used for now.
//!
//! # Safety Invariants — Lock Ordering & Deadlock Prevention
//!
//! Lock nesting must always follow this outermost-to-innermost order.
//! Violating this order risks deadlock.
//!
//! ```text
//! Level 0 (outermost): IRQ disable (irq_save / irq)
//!   └── Level 1: preempt_disable
//!         ├── Level 2a: GRQ lock (sched/sched.rs)
//!         │     └── Level 3a: per-zone lock (mm/zone.rs)
//!         │     └── Level 3b: futex hash bucket lock (sync/futex.rs)
//!         │           └── Level 4: waiter slot lock
//!         ├── Level 2b: process tree lock (process/task.rs)
//!         ├── Level 2c: inode lock (fs/)
//!         └── Level 2d: dentry cache lock (fs/)
//! ```
//!
//! - **INV-LOCK-1**: `preempt_disable` must precede any spinlock acquire.
//!
//! - **INV-LOCK-2**: `irq_save` (or `irq`) must precede `preempt_disable`
//!   when both are needed.
//!
//! - **INV-LOCK-3**: Release order is the reverse: unlock → preempt_enable →
//!   irq_restore.
//!
//! - **INV-LOCK-4**: No lock acquisition cycles across levels (deadlock freedom).
//!   In particular, a Level-3 lock must never be acquired while holding a
//!   different Level-2 lock.
//!
//! - **INV-LOCK-5**: GRQ lock and futex hash bucket lock nesting direction is
//!   consistent: GRQ may nest inside futex bucket, but never the reverse.
//!
//! - **INV-LOCK-6** (virtio-blk ABBA fix): a wait-queue lock
//!   (`WaitQueueHead`'s internal `Spinlock<Vec<WaitQueueEntry>>`) must NEVER
//!   be acquired while holding a virtio driver lock (`VIRTIO_PCI_BLK_LOCK`,
//!   the MMIO device's `virtqueue` lock, `VIRTIO_*_PENDING`). Wait-queue
//!   wakeups run `sched::wake_up_process` (GRQ lock) per waiter while
//!   holding the wait-queue lock, so nesting it under a virtio lock builds
//!   the BLK→waitqueue convoy that froze GNOME final6 (deadlock watchdog:
//!   one CPU stuck on the virtio BSS lock, another on a heap wait-queue
//!   lock, timer IRQs stopped). Drivers must COLLECT completed work under
//!   their locks and deliver wakes OUTSIDE them (timer.rs R12-3 pattern);
//!   see drivers/virtio/mod.rs and `assert_no_virtio_lock`.

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU32, Ordering};
use core::sync::atomic::{AtomicU64, AtomicUsize};

// ==================== Acquire evidence (HEAL-GATE, 122nd) ====================

/// HEAL-GATE (122nd, runq-strand follow-up): per-CPU acquire-side
/// evidence for the tick-side PREEMPT-HEAL family.
///
/// The runq-strand conviction (rs2): the heal's 200-consecutive-
/// observation bar cannot distinguish a LEAKED count from a
/// LEGITIMATE waiter inside a slow acquire — the rs2 waits were on
/// the stalled binder OWNER-LATCH chain (latch_age 2820ms), and the
/// 20.5s heal storm zeroed live waiters' counts mid-acquire
/// (was=0x10001/0x10003), manufacturing phantom counts and (pre
/// preemptible-TTAS) permanent CPU monopolists. This stamp is the
/// acquire-side half of the fix: a waiter inside the CONTENDED TTAS
/// loop publishes (its Task pointer, jiffies) on its own CPU's slot
/// once per SPIN_PREEMPT_PERIOD (64K) iterations — the same 64K
/// boundary the preempt-exit samples need_resched at, so the stamp
/// rides the existing heartbeat (~0.07-2ms native/KVM, well inside
/// one tick). The tick-side heal consults the slot via
/// [`acquire_evidence_fresh`] before zeroing anything: a count with
/// fresh acquire evidence is a LIVE waiter, not a leak, and must
/// never be healed.
///
/// Concurrency argument (all Relaxed): the only writer is the CPU's
/// current spinner inside lock()'s contended loop, and the only
/// reader is the tick IRQ on that SAME CPU — the tick interrupts the
/// loop, so the pair never runs concurrently and per-word coherence
/// is enough (same shape as the TICK_ENTRY_PC stamps). A coupled
/// spinner (preempt_count >= 1) cannot migrate or be preempted, so
/// the stamp's task identity is stable for the whole contended wait.
pub struct AcquireEvidence {
    /// Task pointer of the spinner that last stamped this CPU's slot
    /// (0 = never stamped). Written by the spinner, read by its own
    /// CPU's tick.
    pub task: AtomicUsize,
    /// Jiffies at the last 64K-boundary stamp. Age is only meaningful
    /// together with a matching `task`.
    pub jiffies: AtomicU64,
}

/// Freshness bound for acquire evidence, in jiffies. A live spinner
/// refreshes the stamp every ~0.07-2ms (KVM/native; TCG ~10x slower),
/// so 10 jiffies (100ms at 100Hz) is a 50-1300x margin over the
/// refresh period while staying 20x UNDER the heal's own 200-tick
/// (2s) bar — a leaked count is delayed by at most this window past
/// its last genuine contended spin.
pub const ACQUIRE_EVIDENCE_FRESH_JIF: u64 = 10;

/// The per-CPU acquire-evidence slots (see [`AcquireEvidence`]).
pub static ACQUIRE_EVIDENCE: [AcquireEvidence; crate::config::MAX_CPUS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: AcquireEvidence = AcquireEvidence {
        task: AtomicUsize::new(0),
        jiffies: AtomicU64::new(0),
    };
    [ZERO; crate::config::MAX_CPUS]
};

/// Publish acquire-side evidence from lock()'s CONTENDED loop (called
/// once per SPIN_PREEMPT_PERIOD iterations; the uncontended fast path
/// never executes this). Costs one per-CPU slot read (the SAME
/// cpu_state().current word the tick-side heal adjudicates against —
/// writer and reader are symmetric by construction, no tp-vs-slot
/// skew possible), one jiffies read and two relaxed local stores per
/// 64K spins.
#[inline]
pub fn note_acquire_evidence() {
    let cpu = crate::arch::smp::cpu_id() as usize;
    if cpu >= ACQUIRE_EVIDENCE.len() {
        return;
    }
    // Same-CPU plain read of the scheduler's published current pointer
    // (the spinner cannot be migrated or preempted mid-loop, so this
    // is stable for the whole contended wait).
    let t = crate::sched::sched::cpu_state(cpu).current as usize;
    if t == 0 {
        return;
    }
    let ev = &ACQUIRE_EVIDENCE[cpu];
    ev.task.store(t, Ordering::Relaxed);
    ev.jiffies
        .store(crate::drivers::timer::get_jiffies(), Ordering::Relaxed);
}

/// Does `task` (a Task pointer) have LIVE acquire-side evidence on
/// `cpu` — i.e. was it observed inside lock()'s contended loop on
/// that CPU within [`ACQUIRE_EVIDENCE_FRESH_JIF`] jiffies of `now`?
/// Called from the tick-side PREEMPT-HEAL adjudication: a `true`
/// result means the incumbent's base count is acquire-coupled and
/// live (a slow legitimate waiter), and the heal must be refused.
#[inline]
pub fn acquire_evidence_fresh(cpu: usize, task: usize, now: u64) -> bool {
    if cpu >= ACQUIRE_EVIDENCE.len() {
        return false;
    }
    let ev = &ACQUIRE_EVIDENCE[cpu];
    ev.task.load(Ordering::Relaxed) == task
        && now.wrapping_sub(ev.jiffies.load(Ordering::Relaxed))
            <= ACQUIRE_EVIDENCE_FRESH_JIF
}

// ==================== RawSpinlock (TTAS) ====================

/// Capture this function's return address (the spinlock call site) for
/// deadlock diagnostics.
#[inline]
fn caller_return_address() -> usize {
    let ra: usize;
    #[cfg(feature = "riscv64")]
    // SAFETY: pure register read, no memory access or side effects.
    unsafe {
        core::arch::asm!("mv {}, ra", out(reg) ra, options(nomem, nostack));
    }
    #[cfg(feature = "x86_64")]
    // SAFETY: frame pointers are forced on, so [rbp+8] is the return
    // address of this inlined function's caller.
    unsafe {
        core::arch::asm!("mov {}, qword ptr [rbp + 8]", out(reg) ra, options(nomem, nostack));
    }
    #[cfg(not(any(feature = "riscv64", feature = "x86_64")))]
    {
        ra = 0;
    }
    ra
}

pub struct RawSpinlock {
    locked: AtomicU32,
    /// Deadlock diagnostics: holder's hart id + 1 (0 = free).  Written
    /// AFTER acquiring / cleared BEFORE releasing, so a concurrent reader
    /// may briefly see a stale value — fine for the watchdog print, which
    /// only needs "who held it when everything wedged".
    #[cfg(feature = "dfx-lock-owner")]
    owner: AtomicUsize,
    /// oh-crash 91st (leak hunt): LAST-acquire forensics — pid + call
    /// site + jiffies of the most recent acquire. NOT cleared on unlock
    /// (they name the acquirer even after a leaked/dropped unlock), at
    /// one relaxed store each on the acquire path.
    #[cfg(feature = "dfx-lock-owner")]
    acq_pid: AtomicU32,
    #[cfg(feature = "dfx-lock-owner")]
    acq_site: AtomicUsize,
    #[cfg(feature = "dfx-lock-owner")]
    acq_jiffies: AtomicU64,
}

impl RawSpinlock {
    #[inline]
    pub const fn new() -> Self {
        Self {
            locked: AtomicU32::new(0),
            #[cfg(feature = "dfx-lock-owner")]
            owner: AtomicUsize::new(0),
            #[cfg(feature = "dfx-lock-owner")]
            acq_pid: AtomicU32::new(0),
            #[cfg(feature = "dfx-lock-owner")]
            acq_site: AtomicUsize::new(0),
            #[cfg(feature = "dfx-lock-owner")]
            acq_jiffies: AtomicU64::new(0),
        }
    }

    /// Record the holder (dfx-lock-owner feature only).
    #[inline]
    #[cfg(feature = "dfx-lock-owner")]
    fn record_owner(&self, ra: usize) {
        // cpu_id() is u64 on x86_64 / usize elsewhere — cast to the
        // owner slot's usize.
        self.owner.store((crate::arch::smp::cpu_id() + 1) as usize, Ordering::Relaxed);
        // Leak forensics (see field docs): last acquirer's pid + site.
        self.acq_pid.store(crate::process::current_pid(), Ordering::Relaxed);
        self.acq_site.store(ra, Ordering::Relaxed);
        self.acq_jiffies
            .store(crate::drivers::timer::get_jiffies(), Ordering::Relaxed);
    }

    /// Clear the holder (dfx-lock-owner feature only).
    #[inline]
    #[cfg(feature = "dfx-lock-owner")]
    fn clear_owner(&self) {
        self.owner.store(0, Ordering::Relaxed);
    }

    #[inline]
    #[cfg(not(feature = "dfx-lock-owner"))]
    fn record_owner(&self, _ra: usize) {}

    #[inline]
    #[cfg(not(feature = "dfx-lock-owner"))]
    fn clear_owner(&self) {}

    /// Spinlock deadlock threshold (iterations before warning).
    /// On SMP with QEMU emulation, brief contention is normal — PLIC IRQ
    /// claim/release, GRQ lock, etc. can take 10-100ms of spin time.
    /// 100M iterations ≈ 100-500ms depending on load/CAS latency.
    const DEADLOCK_WARN_ITERS: u32 = 100_000_000;

    /// RUNQ-STRAND fix (121st): preempt-exit sampling period, in spin
    /// iterations. Power of two so the check is a mask. At ~1-30ns per
    /// TTAS iteration (native/KVM) this probes need_resched every
    /// ~0.07-2ms — inside one tick period, so a preemptible spinner
    /// yields within a single 10ms slice. The check itself is one
    /// branch on a mask plus (rarely) a few loads; the uncontended
    /// fast path never executes any of this.
    const SPIN_PREEMPT_PERIOD: u32 = 65_536;

    /// RUNQ-STRAND fix (121st): may this spinner call schedule()?
    ///
    /// Every in-kernel acquire of a raw spinlock is preempt-coupled
    /// (Spinlock guards, GrqGuard, KeyedIrqGuard all add PREEMPT_OFFSET
    /// around the raw claim), so `preempt_count() == 0` proves the
    /// waiter holds NO spinlock at any nesting level and sits in no
    /// interrupt/softirq nesting. The two producers of a count==0
    /// spinner are (a) the tick-side PREEMPT-HEAL zeroing the count of
    /// a task legitimately inside a slow acquire (a false heal — the
    /// 121st census: accesstoken_ser/wallpaper_servi/netmanager spun
    /// at RawSpinlock::lock+0x52 for 500+s with pc=0 and need_resched=1
    /// after heals, each permanently monopolizing its CPU), and (b) a
    /// future uncoupled raw caller. For both, rescheduling from the
    /// WAIT loop is safe: nothing is acquired yet (the TTAS retry is
    /// idempotent across a context switch), and the guards exclude the
    /// deleted preemptible-wait's lost-park hazard (state must still be
    /// RUNNING — a wait path that prepared INTERRUPTIBLE before a
    /// guarded op must not sleep inside the acquire) and irq-off
    /// contexts. This is the reference's preempt_schedule_irq contract
    /// (a preemptible kernel loop must honor need_resched), placed at
    /// the only point where the full guard set is checkable.
    fn spin_may_preempt() -> bool {
        crate::sched::need_resched()
            && crate::interrupt::preempt::preempt_count() == 0
            && crate::arch::cpu::get_interrupts_state()
            && crate::sched::current()
                .map(|t| {
                    t.state().bits() == crate::process::task::TaskState::RUNNING
                })
                .unwrap_or(false)
    }

    #[inline(never)]
    pub fn lock(&self) {
        // Capture caller's return address before spinning
        let caller_ra = caller_return_address();
        let mut spins: u32 = 0;
        // TTAS (test-and-test-and-set, C7): one CAS to acquire; while
        // contended, spin on a plain Relaxed load until the word reads
        // free, then retry the CAS. Relaxed is sufficient for the spin
        // load: it only decides WHEN to retry — the acquiring CAS pairs
        // with the holder's Release store in unlock() and provides all
        // necessary synchronization. The old tight CAS loop kept issuing
        // an atomic RMW on every iteration, hammering the cacheline (and
        // the QEMU TCG atomic slow path) for the whole hold duration.
        loop {
            if self.locked.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_ok() {
                break;
            }
            while self.locked.load(Ordering::Relaxed) != 0 {
                spins = spins.wrapping_add(1);
                // RUNQ-STRAND fix: a provably-preemptible waiter must not
                // monopolize its CPU until the holder releases — yield
                // once per period when the tick asked us to. schedule()
                // clears need_resched at its entry, so the loop cannot
                // re-fire recursively out of the scheduler's own locks.
                if spins & (Self::SPIN_PREEMPT_PERIOD - 1) == 0 {
                    // HEAL-GATE (122nd): publish acquire-side evidence at
                    // the same 64K boundary — the tick-side PREEMPT-HEAL
                    // refuses to zero any count whose owner is inside
                    // this loop RIGHT NOW (a slow legitimate waiter on a
                    // stalled holder chain is not a leak). Contended path
                    // only; the fast path never reaches here.
                    note_acquire_evidence();
                    if Self::spin_may_preempt() {
                        crate::sched::schedule();
                        spins = 0;
                        continue; // re-observe the lock word in the loop head
                    }
                }
                if spins == Self::DEADLOCK_WARN_ITERS {
                    Self::deadlock_warn(self as *const Self, caller_ra);
                    spins = 0; // continue spinning (might resolve)
                }
                core::hint::spin_loop();
            }
        }
        // Diagnostics: record the holder AFTER the acquire (a spinner's
        // watchdog may read a stale 0 in the tiny window — acceptable).
        self.record_owner(caller_ra);
    }

    /// Print deadlock warning via SBI (works even with interrupts disabled).
    fn deadlock_warn(lock_addr: *const Self, caller_ra: usize) {
        // Use SBI putchar directly — printk might need locks we're spinning on
        let cpu = crate::arch::smp::cpu_id();
        let msg = b"DEADLOCK: spinlock stuck cpu=";
        for &b in msg {
            unsafe { crate::console::putchar_no_lock(b); }
        }
        // Print CPU id as decimal digit
        if cpu < 10 {
            unsafe { crate::console::putchar_no_lock(b'0' + cpu as u8); }
        }
        // Print lock address in hex
        let msg2 = b" lock=0x";
        for &b in msg2 {
            unsafe { crate::console::putchar_no_lock(b); }
        }
        let addr = lock_addr as usize;
        let mut shift = (core::mem::size_of::<usize>() * 8) as i32;
        while shift > 0 {
            shift -= 4;
            let nibble = (addr >> (shift as usize)) & 0xF;
            let c = if nibble < 10 { b'0' + nibble as u8 } else { b'a' + (nibble - 10) as u8 };
            unsafe { crate::console::putchar_no_lock(c); }
        }

        // Print caller return address (ra) for debugging
        let msg3 = b" ra=0x";
        for &b in msg3 {
            unsafe { crate::console::putchar_no_lock(b); }
        }
        let mut shift = (core::mem::size_of::<usize>() * 8) as i32;
        while shift > 0 {
            shift -= 4;
            let nibble = (caller_ra >> shift) & 0xF;
            let c = if nibble < 10 { b'0' + nibble as u8 } else { b'a' + (nibble - 10) as u8 };
            unsafe { crate::console::putchar_no_lock(c); }
        }

        // Holder identity: owner = hart+1 (0 = free / mid-handoff).
        // oh-crash 91st extension (the meltdown holder hunt): the owner
        // CPU's current task (pid/comm), the heap allocation CONTAINING
        // the lock (small-object span + first words for offline
        // symbolization of any ops/vtable pointers), and one REMOTE-BT
        // of the holder per unique lock address per boot — the IPI
        // handler on the holder captures its live interrupted stack.
        #[cfg(feature = "dfx-lock-owner")]
        {
            let msg4 = b" holder=";
            for &b in msg4 {
                unsafe { crate::console::putchar_no_lock(b); }
            }
            // SAFETY: lock_addr is a valid RawSpinlock pointer (from lock()).
            let holder = unsafe { (&*lock_addr).owner.load(Ordering::Relaxed) };
            let mut holder_cpu: Option<usize> = None;
            if holder == 0 {
                let msg5 = b"switching";
                for &b in msg5 {
                    unsafe { crate::console::putchar_no_lock(b); }
                }
            } else {
                let h = holder - 1;
                holder_cpu = Some(h);
                if h < 10 {
                    unsafe { crate::console::putchar_no_lock(b'0' + h as u8); }
                }
                // Holder CPU's current task: pid + comm (best-effort —
                // an irqsave holder cannot migrate, so this is the task
                // that acquired; plain lock() holders may have been
                // preempted, in which case the remote-BT below shows
                // what the CPU runs NOW and the pid names the victim).
                // SAFETY: cpu_state(h).current is the scheduler's own
                // published pointer; read-only access.
                let cur = unsafe { crate::sched::sched::cpu_state(h).current };
                if !cur.is_null() {
                    // SAFETY: read-only access to a live Task.
                    let pid = unsafe { (*cur).pid() };
                    let msg_pid = b" task=";
                    for &b in msg_pid {
                        unsafe { crate::console::putchar_no_lock(b); }
                    }
                    Self::putdec(pid as u64);
                    // SAFETY: comm is a fixed [u8;16], no allocation.
                    let comm = unsafe { (*cur).comm() };
                    unsafe { crate::console::putchar_no_lock(b'('); }
                    let mut any = false;
                    for &b in comm.iter() {
                        if b == 0 {
                            break;
                        }
                        if (0x20..0x7f).contains(&b) {
                            unsafe { crate::console::putchar_no_lock(b); }
                            any = true;
                        }
                    }
                    if !any {
                        unsafe { crate::console::putchar_no_lock(b'?'); }
                    }
                    unsafe { crate::console::putchar_no_lock(b')'); }
                }
            }

        unsafe { crate::console::putchar_no_lock(b'\n'); }

        // Leak forensics (dfx-lock-owner): the LAST acquirer's pid,
        // call site and acquire age — these persist past a cleared
        // owner word, so a leaked lock names who took it and when
        // even when the owner stamp is mid-handoff. Plus the raw
        // locked word (1 = genuinely held; anything else is a torn
        // or corrupted state worth seeing).
        #[cfg(feature = "dfx-lock-owner")]
        {
            // SAFETY: lock_addr is a valid RawSpinlock pointer (from lock()).
            let l = unsafe { &*lock_addr };
            let apid = l.acq_pid.load(Ordering::Relaxed);
            let asite = l.acq_site.load(Ordering::Relaxed);
            let ajif = l.acq_jiffies.load(Ordering::Relaxed);
            let wv = l.locked.load(Ordering::Relaxed);
            let now = crate::drivers::timer::get_jiffies();
            let msg = b"LOCK-ACQ: word=";
            for &b in msg {
                unsafe { crate::console::putchar_no_lock(b); }
            }
            Self::puthex(wv as u64);
            let msg = b" pid=";
            for &b in msg {
                unsafe { crate::console::putchar_no_lock(b); }
            }
            Self::putdec(apid as u64);
            let msg = b" site=0x";
            for &b in msg {
                unsafe { crate::console::putchar_no_lock(b); }
            }
            Self::puthex(asite as u64);
            let msg = b" age=";
            for &b in msg {
                unsafe { crate::console::putchar_no_lock(b); }
            }
            Self::putdec(now.wrapping_sub(ajif));
            let msg = b"j\n";
            for &b in msg {
                unsafe { crate::console::putchar_no_lock(b); }
            }
        }

        // Containing heap object (works with or without a known holder):
        // slab span + the first 6 words of the object. Any word landing
        // in kernel text (0xffffffff8...) symbolizes offline against
        // gen-ksyms output — that names the lock's OWNER STRUCT (ops
        // table / vtable / function pointer members).
        if let Some((base, sz)) = crate::mm::small_alloc::object_span(lock_addr as usize) {
            let msg = b"LOCK-OBJ: small kmalloc-";
            for &b in msg {
                unsafe { crate::console::putchar_no_lock(b); }
            }
            Self::putdec(sz as u64);
            let msg = b" base=0x";
            for &b in msg {
                unsafe { crate::console::putchar_no_lock(b); }
            }
            Self::puthex(base as u64);
            let msg = b" words=";
            for &b in msg {
                unsafe { crate::console::putchar_no_lock(b); }
            }
            for i in 0..6 {
                // SAFETY: diagnostic read of the object's first words;
                // the span was validated by object_span (inside a live
                // slab page, in-bounds of the object's slot).
                let w = unsafe { ((base + i * 8) as *const u64).read_unaligned() };
                Self::puthex(w);
                if i < 5 {
                    unsafe { crate::console::putchar_no_lock(b','); }
                }
            }
            unsafe { crate::console::putchar_no_lock(b'\n'); }
        } else if let Some((base, order, free)) =
            crate::mm::buddy_allocator::GLOBAL_ALLOCATOR.buddy_block_span(lock_addr as usize)
        {
            // oh-crash 98th: DIRECT BUDDY blocks (>1KB kmalloc — Task
            // structs, large fs/ipc objects). Same offline-symbolization
            // trick on the first words; `free!=0` (a lock inside a FREE
            // block) is itself evidence of a freed-with-lock-reference
            // object, printed as words=FREE.
            let msg = b"LOCK-OBJ: buddy order-";
            for &b in msg {
                unsafe { crate::console::putchar_no_lock(b); }
            }
            Self::putdec(order as u64);
            let msg = b" base=0x";
            for &b in msg {
                unsafe { crate::console::putchar_no_lock(b); }
            }
            Self::puthex(base as u64);
            let msg = b" free=";
            for &b in msg {
                unsafe { crate::console::putchar_no_lock(b); }
            }
            Self::putdec(free as u64);
            let msg = b" words=";
            for &b in msg {
                unsafe { crate::console::putchar_no_lock(b); }
            }
            for i in 0..6 {
                // SAFETY: diagnostic read of the block's first words;
                // buddy_block_span validated the address against the
                // heap window and the block's base alignment.
                let w = unsafe { ((base + i * 8) as *const u64).read_unaligned() };
                Self::puthex(w);
                if i < 5 {
                    unsafe { crate::console::putchar_no_lock(b','); }
                }
            }
            unsafe { crate::console::putchar_no_lock(b'\n'); }
        }

        // One REMOTE-BT per unique lock address per boot: the IPI
        // handler on the holder CPU captures the live interrupted
        // stack. (x86_64 only — the riscv64/aarch64 twins have no
        // IRQ_WORK remote capture yet.)
        #[cfg(feature = "x86_64")]
        {
            static LAST_BT_LOCK: core::sync::atomic::AtomicUsize =
                core::sync::atomic::AtomicUsize::new(0);
            let la = lock_addr as usize;
            let prev = LAST_BT_LOCK.load(Ordering::Acquire);
            if prev != la
                && LAST_BT_LOCK
                    .compare_exchange(prev, la, Ordering::AcqRel, Ordering::Acquire)
                    .is_ok()
            {
                if let Some(h) = holder_cpu {
                    let _ = crate::arch::ipi::remote_backtrace(h, "deadlock-holder");
                }
            }
        }
        }
        #[cfg(not(feature = "dfx-lock-owner"))]
        {
            unsafe { crate::console::putchar_no_lock(b'\n'); }

            // The heap-object span dump is cheap and holder-independent:
            // keep it on default builds too (the meltdown hunts run
            // stock feature sets as controls).
            if let Some((base, sz)) = crate::mm::small_alloc::object_span(lock_addr as usize) {
                let msg = b"LOCK-OBJ: small kmalloc-";
                for &b in msg {
                    unsafe { crate::console::putchar_no_lock(b); }
                }
                Self::putdec(sz as u64);
                let msg = b" base=0x";
                for &b in msg {
                    unsafe { crate::console::putchar_no_lock(b); }
                }
                Self::puthex(base as u64);
                unsafe { crate::console::putchar_no_lock(b'\n'); }
            } else if let Some((base, order, free)) =
                crate::mm::buddy_allocator::GLOBAL_ALLOCATOR.buddy_block_span(lock_addr as usize)
            {
                let msg = b"LOCK-OBJ: buddy order-";
                for &b in msg {
                    unsafe { crate::console::putchar_no_lock(b); }
                }
                Self::putdec(order as u64);
                let msg = b" base=0x";
                for &b in msg {
                    unsafe { crate::console::putchar_no_lock(b); }
                }
                Self::puthex(base as u64);
                let msg = b" free=";
                for &b in msg {
                    unsafe { crate::console::putchar_no_lock(b); }
                }
                Self::putdec(free as u64);
                unsafe { crate::console::putchar_no_lock(b'\n'); }
            }
        }

        // dfx=watchdog (dfx-migrate: rux:taskdump kind=WATCHDOG):
        // follow the warning with a full task-state snapshot
        // (silent-wedge diagnosis — CPUs idle + sleepers never woken).
        if crate::trace::EVENT_TASKDUMP.armed() {
            crate::trace::trace_taskdump(2, 0);
            crate::dfx::taskdump::dump_all_tasks("deadlock-watchdog");
        }
    }

    /// Lockless hex print (deadlock diagnostics — putchar path only).
    fn puthex(v: u64) {
        let mut shift: i32 = 64;
        while shift > 0 {
            shift -= 4;
            let nibble = ((v >> shift) & 0xF) as u8;
            let c = if nibble < 10 { b'0' + nibble } else { b'a' + nibble - 10 };
            unsafe { crate::console::putchar_no_lock(c); }
        }
    }

    /// Lockless decimal print (deadlock diagnostics).
    fn putdec(mut v: u64) {
        let mut buf = [0u8; 20];
        let mut i = 0;
        loop {
            buf[i] = b'0' + (v % 10) as u8;
            i += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        while i > 0 {
            i -= 1;
            unsafe { crate::console::putchar_no_lock(buf[i]); }
        }
    }

    #[inline]
    pub fn try_lock(&self) -> bool {
        let ok = self.locked.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_ok();
        if ok {
            self.record_owner(caller_return_address());
        }
        ok
    }

    #[inline]
    pub fn unlock(&self) {
        self.clear_owner();
        self.locked.store(0, Ordering::Release);
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Acquire) != 0
    }

    #[inline]
    pub unsafe fn reset(&mut self) {
        self.clear_owner();
        self.locked.store(0, Ordering::Release);
    }
}

// ==================== Spinlock<T> ====================

impl Spinlock<i32> {
    /// Offset of the data word inside this Spinlock layout (the compiler
    /// reorders raw/data — diagnostic watchpoints must use this, never
    /// fixed arithmetic).
    pub fn data_offset() -> usize {
        core::mem::offset_of!(Spinlock<i32>, data)
    }
}

pub struct Spinlock<T: ?Sized> {
    raw: RawSpinlock,
    data: UnsafeCell<T>,
}

// SAFETY: Spinlock<T> only allows &mut T access via the guard, which exists
// only while the inner lock is held.  Send is required so the lock (and its
// data) can be moved between threads; Sync is safe because the lock mediates
// all access.
unsafe impl<T: ?Sized + Send> Send for Spinlock<T> {}
unsafe impl<T: ?Sized + Send> Sync for Spinlock<T> {}

impl<T> Spinlock<T> {
    #[inline]
    pub const fn new(data: T) -> Self {
        Self { raw: RawSpinlock::new(), data: UnsafeCell::new(data) }
    }

    /// Preempt disable + lock.
    /// Guard drop: unlock + preempt enable.
    /// track_caller: leakorigin's outermost-acquire shadow records the
    /// LOCK CALLER's site (preempt-hunt), not this helper's line.
    ///
    /// Reference discipline (sync-realign, 2026-11): a spinlock NEVER
    /// sleeps. The acquire is preempt_disable + pure bounded spinning
    /// (spin until acquired — the DEADLOCK_WARN threshold only prints,
    /// it never gives up); no task-state change and no schedule()
    /// anywhere in this path. This is sound ONLY because every lock
    /// that may be held across blocking I/O is a SleepMutex
    /// (kernel/src/sync/mutex.rs), never a Spinlock: a preempt-disabled
    /// spinner can starve the holder's wake IPI, so a Spinlock section
    /// must stay short by construction. The former "preemptible
    /// bounded wait" that yielded via schedule() here (preempt-hunt,
    /// then the lost-park-patched handoff) was a divergence from the
    /// reference design and is deleted — the lost-park hazard class
    /// (a caller inside a wait path's prepared window parked inside a
    /// spinlock acquire) is structurally impossible now.
    ///
    /// RUNQ-STRAND fix (121st) precision on that discipline: the raw
    /// TTAS loop below CAN call schedule(), but only through
    /// `spin_may_preempt()` — which requires preempt_count()==0 and is
    /// therefore unreachable for THIS acquire (our own preempt_disable
    /// guarantees count >= 1 for every Spinlock-guarded caller). The
    /// exit exists solely for a waiter whose count was zeroed under it
    /// (tick-side PREEMPT-HEAL false positive) or a future uncoupled
    /// raw caller; see `spin_may_preempt` for the full guard set.
    #[inline]
    #[track_caller]
    pub fn lock(&self) -> SpinlockGuard<'_, T> {
        preempt_disable();
        self.raw.lock();
        SpinlockGuard { lock: self }
    }

    /// Disable interrupts + preempt disable + lock.
    /// Guard drop: unlock + preempt enable + restore interrupts.
    #[inline]
    #[track_caller]
    pub fn lock_irq(&self) -> SpinlockIrqGuard<'_, T> {
        let flags = irq_save();
        preempt_disable_at(core::panic::Location::caller());
        self.raw.lock();
        SpinlockIrqGuard { lock: self, flags }
    }

    /// Save interrupt state + disable + preempt disable + lock.
    /// Guard drop: unlock + preempt enable + restore interrupt state.
    #[inline]
    #[track_caller]
    pub fn lock_irqsave(&self) -> SpinlockIrqGuard<'_, T> {
        let flags = irq_save();
        preempt_disable_at(core::panic::Location::caller());
        self.raw.lock();
        SpinlockIrqGuard { lock: self, flags }
    }

    /// Disable bottom-half (softirq) + lock.
    /// bh_disable() increments preempt_count by SOFTIRQ_OFFSET,
    /// which also disables preemption.
    /// Guard drop: unlock + bh_enable (decrements preempt_count).
    #[inline]
    pub fn lock_bh(&self) -> SpinlockBhGuard<'_, T> {
        bh_disable();
        self.raw.lock();
        SpinlockBhGuard { lock: self }
    }

    #[inline]
    #[track_caller]
    pub fn try_lock(&self) -> Option<SpinlockGuard<'_, T>> {
        preempt_disable_at(core::panic::Location::caller());
        if self.raw.try_lock() {
            Some(SpinlockGuard { lock: self })
        } else {
            preempt_enable();
            None
        }
    }

    #[inline]
    #[track_caller]
    pub fn try_lock_irqsave(&self) -> Option<SpinlockIrqGuard<'_, T>> {
        let flags = irq_save();
        preempt_disable_at(core::panic::Location::caller());
        if self.raw.try_lock() {
            Some(SpinlockIrqGuard { lock: self, flags })
        } else {
            preempt_enable();
            irq_restore(flags);
            None
        }
    }

    #[inline]
    pub fn is_locked(&self) -> bool { self.raw.is_locked() }

    /// Get a shared reference to the inner data without locking.
    ///
    /// # Safety
    /// Caller must ensure no concurrent mutable access (e.g., data is
    /// write-once during boot and only read thereafter).
    #[inline]
    pub unsafe fn get_ref(&self) -> &T {
        &*self.data.get()
    }

    /// Get a mutable reference to the inner data without locking.
    ///
    /// # Safety
    /// Caller must ensure exclusive access (e.g., no concurrent readers or writers).
    #[inline]
    pub unsafe fn get_mut_unchecked(&self) -> &mut T {
        &mut *self.data.get()
    }

    /// Consume the lock and return the inner data.
    ///
    /// # Safety
    /// Caller must ensure no other thread holds a reference to the lock or data.
    #[inline]
    pub unsafe fn into_inner(self) -> T { self.data.into_inner() }
}

// ==================== SpinlockGuard (plain) ====================

pub struct SpinlockGuard<'a, T: ?Sized> {
    lock: &'a Spinlock<T>,
}

// SAFETY: SpinlockGuard exists only while the lock is held, so the guarded
// data cannot be accessed concurrently from another thread.
unsafe impl<T: ?Sized + Send> Send for SpinlockGuard<'_, T> {}

impl<T: ?Sized> Deref for SpinlockGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: Guard holds the lock — no concurrent &mut access possible.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for SpinlockGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Guard holds the lock exclusively — no other access possible.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for SpinlockGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.unlock();
        // Leak-hunt: pass the lock instance so the underflow report can
        // name WHICH Spinlock double-dropped (static locks symbolize
        // against the ELF; heap locks identify the owning object).
            crate::interrupt::preempt::preempt_count_sub2_at(
                crate::interrupt::preempt::PREEMPT_OFFSET,
                core::panic::Location::caller(),
                // RawSpinlock is Spinlock's first field, so its address
                // IS the Spinlock instance's address (T may be ?Sized,
                // making &Spinlock<T> a fat reference — go through the
                // thin raw field pointer).
                &self.lock.raw as *const RawSpinlock as usize,
            );
    }
}

// ==================== SpinlockIrqGuard (irqsave) ====================

pub struct SpinlockIrqGuard<'a, T: ?Sized> {
    lock: &'a Spinlock<T>,
    flags: bool,
}

// SAFETY: SpinlockIrqGuard exists only while the lock is held, so the
// guarded data cannot be accessed concurrently.
unsafe impl<T: ?Sized + Send> Send for SpinlockIrqGuard<'_, T> {}

impl<T: ?Sized> Deref for SpinlockIrqGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: Guard holds the lock — no concurrent &mut access possible.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for SpinlockIrqGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Guard holds the lock exclusively — no other access possible.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for SpinlockIrqGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        // ORDER FIX (glib-mutex lane, 2026-11-14): preempt sub BEFORE the
        // raw unlock. The old order exposed a [unlock..sub] window in
        // which the lock was free while the dropping task still carried
        // its +1 — any observer adjudicating "parked task with a live
        // count" (the WAKE-HEAL class) could zero an in-flight guard.
        // preempt_enable is a bare sub (no resched) and IF stays 0 until
        // irq_restore below, so the reorder cannot enable preemption
        // under the lock.
        preempt_enable();
        self.lock.raw.unlock();
        irq_restore(self.flags);
    }
}

impl<T: ?Sized> SpinlockIrqGuard<'_, T> {
    /// Release only the spinlock (preempt_enable + unlock), returning
    /// the saved IRQ flags. The caller must later call `irq_restore(flags)`
    /// to restore interrupt state.
    ///
    /// This is used by the scheduler: we must drop the rq lock before
    /// context_switch but keep interrupts disabled until after
    /// context_switch returns (following Linux's pattern where
    /// finish_task_switch releases the lock).
    ///
    /// ORDER FIX (glib-mutex lane): preempt sub before unlock — see the
    /// Drop impl above; the GRQ twin's capture (garepro26 WAKE-HEAL /
    /// PREEMPT-UNDERFLOW pair) convicted this window.
    #[inline]
    pub fn unlock_irqretain(self) -> bool {
        let flags = self.flags;
        preempt_enable();
        self.lock.raw.unlock();
        core::mem::forget(self); // prevent Drop from running
        flags
    }
}

// ==================== SpinlockBhGuard (bottom-half) ====================

pub struct SpinlockBhGuard<'a, T: ?Sized> {
    lock: &'a Spinlock<T>,
}

// SAFETY: SpinlockBhGuard exists only while the lock is held, so the
// guarded data cannot be accessed concurrently.
unsafe impl<T: ?Sized + Send> Send for SpinlockBhGuard<'_, T> {}

impl<T: ?Sized> Deref for SpinlockBhGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &Self::Target {
        // SAFETY: Guard holds the lock — no concurrent &mut access possible.
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for SpinlockBhGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: Guard holds the lock exclusively — no other access possible.
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for SpinlockBhGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.raw.unlock();
        bh_enable();
    }
}

// ==================== Inline helpers ====================

#[inline]
#[track_caller]
fn preempt_disable() {
    crate::interrupt::preempt::preempt_count_add_at(
        crate::interrupt::preempt::PREEMPT_OFFSET,
        core::panic::Location::caller(),
    );
}

/// As `preempt_disable`, booking the CALLER-of-the-lock's location (the
/// #[track_caller] lock helpers forward their own caller down so the
/// leakorigin shadow names the real acquire site).
#[inline]
fn preempt_disable_at(loc: &'static core::panic::Location<'static>) {
    crate::interrupt::preempt::preempt_count_add_at(
        crate::interrupt::preempt::PREEMPT_OFFSET,
        loc,
    );
}

#[inline]
#[track_caller]
fn preempt_enable() {
    crate::interrupt::preempt::preempt_count_sub_at(
        crate::interrupt::preempt::PREEMPT_OFFSET,
        core::panic::Location::caller(),
    );
}

#[inline]
#[track_caller]
fn irq_save() -> bool {
    // track_caller: syscost's REST histogram sees the outermost lock
    // user instead of this line (no-op unless syscost is armed).
    crate::arch::cpu::save_and_disable_irq()
}

#[inline]
#[track_caller]
fn irq_restore(flags: bool) {
    crate::arch::cpu::restore_irq(flags);
}

#[inline]
#[track_caller]
fn bh_disable() {
    crate::interrupt::preempt::preempt_count_add(
        crate::interrupt::preempt::SOFTIRQ_OFFSET,
    );
}

#[inline]
#[track_caller]
fn bh_enable() {
    crate::interrupt::preempt::preempt_count_sub_at(
        crate::interrupt::preempt::SOFTIRQ_OFFSET,
        core::panic::Location::caller(),
    );
}

// ==================== Free functions (C-style API) ====================

#[inline(always)]
pub fn raw_spin_lock(l: &RawSpinlock) { l.lock(); }
#[inline(always)]
pub fn raw_spin_unlock(l: &RawSpinlock) { l.unlock(); }
#[inline(always)]
pub fn raw_spin_is_locked(l: &RawSpinlock) -> bool { l.is_locked() }
#[inline(always)]
pub fn raw_spin_trylock(l: &mut RawSpinlock) -> bool { l.try_lock() }
