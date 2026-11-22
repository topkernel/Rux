//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Side-file framework edges for the scheduler's non-core modules
//! (industrial-reduction lane L2, coordination chunk, 2026-11-20).
//!
//! L1 owns sched.rs/fair.rs/rt.rs/cgroup.rs/edges.rs; this module holds
//! the edges of the SIDE files L2 claimed (deadline.rs, sched_ev.rs,
//! wakewatch.rs) so those service files carry zero unsafe — same
//! doctrine as `process/unsafe_edges.rs`, one INV contract per edge.

// ---- deadline.rs ---------------------------------------------------------

// SAFETY: DlRunQueue is only ever accessed behind the GRQ (its callers
// hold the global runqueue lock for every enqueue/dequeue/pick); the
// raw Task pointers inside the BTreeMap are guarded by the queue's
// link/unlink protocol.
unsafe impl Send for crate::sched::deadline::DlRunQueue {}
// SAFETY: same GRQ serialization across CPUs.
unsafe impl Sync for crate::sched::deadline::DlRunQueue {}

// ---- sched_ev.rs ----------------------------------------------------------

/// The event ring's reserved slot (writer side).
///
/// INV-SCHED-EVRING-1: `slot` is the modulo-bounded index of a unique
/// fetch_add reservation — this call owns the slot until the publish
/// store; the array is a fixed static; an overrun clobbers the oldest
/// record (counted in EV_LOST, never a correctness object).
#[inline]
pub fn evring_slot_mut<T, const N: usize>(
    ring: &EvRing<T, N>,
    slot: usize,
) -> &mut T {
    debug_assert!(slot < N);
    // SAFETY: INV-SCHED-EVRING-1 — owned reserved slot in the fixed
    // static array.
    unsafe { &mut (*ring.0.get())[slot] }
}

/// The event ring's published slot (reader side).
///
/// INV-SCHED-EVRING-1 (read arm): read-only diagnostic access; the
/// caller validates `pub_seq` before trusting any field.
#[inline]
pub fn evring_slot_ref<T, const N: usize>(
    ring: &EvRing<T, N>,
    slot: usize,
) -> &T {
    debug_assert!(slot < N);
    // SAFETY: INV-SCHED-EVRING-1 — shared diagnostic read of a slot the
    // pub_seq protocol validates.
    unsafe { &(*ring.0.get())[slot] }
}

/// The event-ring substrate (the S1b buffer.rs pattern): the UnsafeCell
/// ring + its Sync proof live FRAMEWORK-side; the policy crate owns the
/// cell type and aliases this generic.
pub struct EvRing<T, const N: usize>(pub core::cell::UnsafeCell<[T; N]>);

impl<T, const N: usize> EvRing<T, N> {
    /// Construct over a fully-initialized backing array (type aliases
    /// do not carry tuple constructors — this is the alias-visible ctor).
    pub const fn new(arr: [T; N]) -> Self {
        Self(core::cell::UnsafeCell::new(arr))
    }
}
// SAFETY: the EvRing slot protocol (fetch_add reservation + pub_seq
// publication) is the same lock-free discipline the printk ring uses;
// at most one writer owns a slot at a time.
unsafe impl<T, const N: usize> Sync for EvRing<T, N> {}
