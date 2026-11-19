//! The scheduler's audited unsafe edges (framework zone).
//!
//! unsafe-slim round L1 (2026-11-20), per the framekernel doctrine: the
//! scheduler's POLICY code (sched.rs, fair.rs, rt.rs, cgroup.rs) is safe
//! by construction — every raw-pointer dereference it needs of the
//! scheduler's pointer currency (`*mut Task` slots/queue entries, the
//! boot-provenance statics) crosses one of the audited functions in
//! THIS file, each carrying an INV-SCE contract. One audit per edge,
//! not per call site; the aliasing discipline is exactly the one the
//! per-site SAFETY comments documented before the consolidation.
//!
//! INV registry (mirrors kernel/src/safety/ + process/task_ref.rs):
//! - INV-SCE-1 `task_mut`    — pointer→exclusive reference, scheduler
//!   ownership disciplines (GRQ hold, IRQs-off switch window, this
//!   CPU's current slot/tp, or a walker pin).
//! - INV-SCE-2 `task_shared` — pointer→shared reference, same liveness
//!   sources, diagnostic/atomic-only reads.
//! - INV-SCE-3 `grq_*`       — the MaybeUninit GRQ boot provenance:
//!   written once by `grq_init` under the GRQ_READY CAS, read only
//!   after the flag's Acquire side.

use crate::process::task::Task;

use super::sched::GlobalRunQueue;

// ---------------------------------------------------------------------------
// INV-SCE-4: the per-CPU idle-task storages
// ---------------------------------------------------------------------------

/// The per-CPU idle-task slots: `static mut` + MaybeUninit because
/// `Task::new_idle_at` is not const. Each slot is written exactly once
/// by `sched::init`'s idle-task construction (BSP, before secondaries
/// schedule); afterwards the slots are read-only (the b81 PER_CPU
/// scribble heal compares them as the static truth).
static mut IDLE_TASK_STORAGES: [core::mem::MaybeUninit<Task>; crate::config::MAX_CPUS] =
    [const { core::mem::MaybeUninit::uninit() }; crate::config::MAX_CPUS];

/// Address of cpu's idle-task slot (INV-SCE-4). Pointer-only: callers
/// keep identity semantics; the slot's address is stable for the boot's
/// lifetime. Valid as a `*mut Task` once the init pass wrote it (all
/// readers run post-init — scheduling requires it).
#[inline]
pub fn idle_task_ptr(cpu: usize) -> *mut Task {
    debug_assert!(cpu < crate::config::MAX_CPUS);
    // SAFETY: INV-SCE-4 — index in range (init writes every slot before
    // any CPU schedules); addr-of forms no reference to possibly-uninit
    // memory.
    unsafe { core::ptr::addr_of_mut!(IDLE_TASK_STORAGES[cpu]).cast::<Task>() }
}

/// Base address of the whole idle-task storage array (INV-SCE-4):
/// address arithmetic only, for the kernel-pointer range check that
/// admits the static Task storages (never their contents).
#[inline]
pub fn idle_task_storage_base() -> usize {
    // SAFETY: INV-SCE-4 — only the static's address is read.
    unsafe { core::ptr::addr_of!(IDLE_TASK_STORAGES) as usize }
}

// ---------------------------------------------------------------------------
// INV-SCE-1 / INV-SCE-2: task pointer → reference
// ---------------------------------------------------------------------------

/// Derive an exclusive task reference from a scheduler-owned pointer.
///
/// # Contract (INV-SCE-1)
/// `p` is non-null and points at a live `Task` whose slot lifetime is
/// covered by one of the scheduler's ownership disciplines:
/// (a) the GRQ hold — queue-linked tasks cannot be freed or picked
///     while the lock is held;
/// (b) this CPU's IRQs-off switch window — `prev`/`next` around the
///     context switch;
/// (c) this CPU's current slot or the hardware `tp` register — the
///     executing task;
/// (d) a walker pin — the PID-hash bucket lock or a parent reference
///     (`pid_hash_for_each_task`'s contract).
/// Mutation through the reference happens only from the context that
/// owns the task under that discipline — the same argument every
/// pre-consolidation call site made inline. Identity comparisons stay
/// on the raw pointer (`p`), never on the reference.
#[inline]
pub fn task_mut(p: *mut Task) -> &'static mut Task {
    debug_assert!(!p.is_null(), "edges::task_mut(null)");
    // SAFETY: INV-SCE-1 (caller-proved liveness/exclusion); the
    // reference is a raw alias of the pointer, like `current()` and
    // `TaskRef::DerefMut` before it.
    unsafe { &mut *p }
}

/// Derive a shared task reference (diagnostic reads, atomics) from a
/// scheduler-owned pointer.
///
/// # Contract (INV-SCE-2)
/// Same liveness sources as [task_mut]; the reference is used for
/// reads only (field reads, atomic loads, safe `&self` methods). A
/// task being read this way may concurrently execute on its owning
/// CPU — reads see mid-flight state, exactly as the raw-pointer reads
/// this edge replaces did.
#[inline]
pub fn task_shared(p: *const Task) -> &'static Task {
    debug_assert!(!p.is_null(), "edges::task_shared(null)");
    // SAFETY: INV-SCE-2.
    unsafe { &*p }
}

// ---------------------------------------------------------------------------
// INV-SCE-3: the GRQ boot provenance
// ---------------------------------------------------------------------------

/// The global run queue. `static mut` + MaybeUninit because
/// `GlobalRunQueue::new()` is not const (BTreeMap): written exactly
/// once by [`grq_init`] under the GRQ_READY CAS, read afterwards.
static mut GRQ: core::mem::MaybeUninit<GlobalRunQueue> = core::mem::MaybeUninit::uninit();

/// Publishing flag for the GRQ's one-time initialization (Acquire on
/// the readers' side pairs with the writer's stage + Release).
static GRQ_READY: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Has the GRQ completed its one-time initialization?
#[inline]
pub fn grq_is_ready() -> bool {
    GRQ_READY.load(core::sync::atomic::Ordering::Acquire)
}

/// The initializing CPU's claim flag for the one-time GRQ setup:
/// `false` when this call won the CAS and must stage + publish.
pub fn grq_claim_init() -> bool {
    GRQ_READY.compare_exchange(
        false,
        true,
        core::sync::atomic::Ordering::AcqRel,
        core::sync::atomic::Ordering::Acquire,
    )
    .is_err()
}

/// Writer-side staging slot (INV-SCE-3): visible only to the CAS-winning
/// initializer between [`grq_claim_init`] and [`grq_publish_init`].
/// Overwrites any prior content (early-boot re-init is not a supported
/// shape; the claim CAS serializes the single writer).
pub fn grq_stage(value: GlobalRunQueue) {
    // SAFETY: INV-SCE-3 writer side — this CPU holds the claim (the
    // GRQ_READY CAS in grq_claim_init); no reader passes the flag
    // before grq_publish_init's Release store.
    unsafe {
        GRQ = core::mem::MaybeUninit::new(value);
    }
}

/// Publish the staged GRQ (INV-SCE-3): the Release store that lets
/// every `grq_ref`/`grq_mut` reader observe the staged value. Also the
/// spinner's exit condition.
pub fn grq_publish_init() {
    GRQ_READY.store(true, core::sync::atomic::Ordering::Release);
}

/// Shared view of the initialized GRQ (INV-SCE-3 read side).
#[inline]
pub fn grq_ref() -> &'static GlobalRunQueue {
    // SAFETY: INV-SCE-3 — the caller passed the GRQ_READY Acquire gate
    // (sched::grq() checks it and spins through grq_init otherwise);
    // the MaybeUninit content is complete and never moves again.
    unsafe { GRQ.assume_init_ref() }
}

/// Exclusive view of the initialized GRQ (INV-SCE-3 mutation side).
/// Callers: the claim-winning initializer between [grq_stage] and
/// [grq_publish_init] (boot provenance fixups), or any GRQ-locked
/// path after the ready gate.
#[inline]
pub fn grq_mut() -> &'static mut GlobalRunQueue {
    // SAFETY: INV-SCE-3 — either the caller holds the init claim (the
    // content was staged by grq_stage immediately before) or passed
    // the ready gate; the reference is used under the GRQ lock or the
    // claim's write exclusion.
    unsafe { GRQ.assume_init_mut() }
}
