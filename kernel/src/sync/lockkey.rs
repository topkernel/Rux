//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Typed lock keys — the lock-order hierarchy as types
//! (docs/architecture/rust-design-headroom.md §3, second slice; the
//! companion of divergence D-7 CpuLocal, which typed the per-CPU
//! interrupt window).
//!
//! The bug class this kills: INV-LOCK-* ordering violations
//! (docs/architecture/lock-ordering.md) are today review-enforced and
//! watchdog-detected — the futex-wake path nests GRQ inside the bucket
//! lock (INV-LOCK-5), and the reverse order deadlocks. Here the
//! permitted nestings are TRAIT IMPLS: acquiring a keyed lock while
//! another is held goes through [`KeyedRawSpinlock::lock_nested`],
//! which only compiles when an `impl CanNestInside<Outer> for Inner`
//! exists. The reverse or same-level acquisition is not a runtime
//! spin-to-deadlock — it does not compile:
//!
//! ```ignore
//! let bucket = KeyedRawSpinlock::<FutexBucketKey>::new();
//! let grq = KeyedRawSpinlock::<GrqKey>::new();
//! let bg = bucket.lock();
//! let gg = grq.lock_nested(&bg);        // OK: INV-LOCK-5 forward
//! drop(gg);
//! let gg2 = grq.lock();
//! let bg2 = bucket.lock_nested(&gg2);   // DOES NOT COMPILE
//!                                       // (no CanNestInside<GrqKey>
//!                                       //  for FutexBucketKey)
//! ```
//!
//! Scope discipline (the same opportunistic doctrine as D-6/D-4):
//! unkeyed spinlocks keep compiling unchanged; a pair migrates to keys
//! when its invariant is named in lock-ordering.md, and the migration
//! must not widen the public API of the migrated subsystem. The first
//! adoption is the GRQ's own lock (`sched::sched::GlobalRunQueue`),
//! typed `GrqKey` internally with its guard API unchanged; the futex
//! bucket side of INV-LOCK-5 (and the INV-LOCK-6 stats/net pair)
//! follow in later slices, at which point the wake path's nesting
//! becomes a compile-checked `lock_nested`.
//!
//! The IRQ-save/preempt discipline is deliberately NOT part of the key:
//! that window is typed by CpuLocal's guard (D-7); the key types only
//! the ORDER fact. One mechanism per concern.

use core::marker::PhantomData;

use super::spinlock::RawSpinlock;

/// A lock-order class. One zero-sized type per named invariant in
/// `docs/architecture/lock-ordering.md`; the class NAME is what the
/// deadlock watchdog prints alongside the type-level story.
pub trait NestKey: 'static + Copy {
    /// The invariant this key participates in (for diagnostics).
    const NAME: &'static str;
}

/// Permitted nesting: `impl CanNestInside<Outer> for Inner` states that
/// a lock of class `Inner` may be acquired while a lock of class
/// `Outer` is held. Absence of the impl is the invariant: the reverse
/// (or same-level) nesting cannot compile.
pub trait CanNestInside<Outer: NestKey>: NestKey {}

// ---- The key classes (lock-ordering.md, INV-LOCK-5/6) -------------

/// The global run queue lock (divergence D-1). INV-LOCK-5: nests INSIDE
/// the futex bucket lock, never the reverse; same-level nesting is a
/// self-deadlock and has no impl.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrqKey;
impl NestKey for GrqKey {
    const NAME: &'static str = "grq";
}
/// GRQ may nest inside the futex bucket lock (INV-LOCK-5 forward:
/// futex_wake -> Task::wake_up -> enqueue_task).
impl CanNestInside<FutexBucketKey> for GrqKey {}

/// A futex hash-bucket lock (`sync::futex::HASH_HEADS`). Nothing may be
/// acquired inside it EXCEPT the classes with explicit impls above —
/// today only `GrqKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FutexBucketKey;
impl NestKey for FutexBucketKey {
    const NAME: &'static str = "futex-bucket";
}

/// Per-socket queue statistics (INV-LOCK-6): stats nest inside the
/// virtio-net rx/tx queue locks, never the reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetStatsKey;
impl NestKey for NetStatsKey {
    const NAME: &'static str = "net-stats";
}
/// INV-LOCK-6 forward: stats acquired while a queue lock is held.
impl CanNestInside<NetQueueKey> for NetStatsKey {}

/// A virtio-net rx/tx queue lock (INV-LOCK-6 outer class).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NetQueueKey;
impl NestKey for NetQueueKey {
    const NAME: &'static str = "net-queue";
}

/// A futex waiter-pool SLOT lock (`sync::futex::WAITER_POOL`). Slot
/// locks are taken inside the bucket lock that owns the chain the slot
/// is linked into (the wait/wake/exit/requeue paths); the allocator
/// and the free path take them top-level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaiterSlotKey;
impl NestKey for WaiterSlotKey {
    const NAME: &'static str = "futex-waiter-slot";
}
/// A slot may be acquired while its bucket lock is held (the shape of
/// every chain walk under a bucket guard). Same-class double-slot
/// acquisition (the requeue relink window) stays on the plain API —
/// the keyed surface fences ACROSS classes.
impl CanNestInside<FutexBucketKey> for WaiterSlotKey {}

// ---- The keyed lock ----------------------------------------------

/// A [`RawSpinlock`] whose position in the lock hierarchy is part of
/// its type. Same size as the raw lock (`PhantomData<K>` is free), the
/// acquire paths inline to the same instructions; the ONLY new API is
/// `lock_nested`, the compile-checked nesting acquisition.
pub struct KeyedRawSpinlock<K: NestKey> {
    raw: RawSpinlock,
    _key: PhantomData<K>,
}

// SAFETY: RawSpinlock's atomic protocol is the synchronization; the
// key is zero-sized and carries no data.
unsafe impl<K: NestKey> Sync for KeyedRawSpinlock<K> {}

impl<K: NestKey> KeyedRawSpinlock<K> {
    pub const fn new() -> Self {
        Self { raw: RawSpinlock::new(), _key: PhantomData }
    }

    /// Top-level acquire (no keyed lock held). The plain call stays
    /// available because most call sites are top-level by construction;
    /// nesting while another keyed lock is held MUST use `lock_nested`.
    #[inline]
    #[track_caller]
    pub fn lock(&self) -> KeyedGuard<'_, K> {
        self.raw.lock();
        KeyedGuard {
            lock: self as *const Self as *mut Self,
            _key: PhantomData,
            _lifetime: PhantomData,
        }
    }

    /// Try-lock variant of the top-level acquire.
    #[inline]
    #[track_caller]
    pub fn try_lock(&self) -> Option<KeyedGuard<'_, K>> {
        if self.raw.try_lock() {
            Some(KeyedGuard {
                lock: self as *const Self as *mut Self,
                _key: PhantomData,
                _lifetime: PhantomData,
            })
        } else {
            None
        }
    }

    /// Claim without returning a guard — for subsystems that wrap the
    /// raw lock in their OWN irqsave/preempt guard protocol around the
    /// lock (the GRQ pattern: its GrqGuard carries a pointer back to
    /// the queue and unlocks through it). Pairs with [`unlock`].
    #[inline]
    #[track_caller]
    pub fn try_lock_claim(&self) -> bool {
        self.raw.try_lock()
    }

    /// Unlock without a guard (for subsystems whose guard wraps the
    /// full irqsave/preempt protocol around the raw lock — the GRQ
    /// pattern). Delegates to the raw protocol; the key neither adds
    /// nor removes anything here.
    #[inline]
    #[track_caller]
    pub fn unlock(&self) {
        self.raw.unlock();
    }
}

impl<Inner: NestKey> KeyedRawSpinlock<Inner> {
    /// Compile-checked nested acquire: take this (`Inner`-class) lock
    /// while an `Outer`-class keyed lock is held. The proof is minted
    /// by the OUTER guard (`guard.proof()`); the call only compiles
    /// when `Inner: CanNestInside<Outer>` — the trait impl list IS the
    /// permitted-nesting table of lock-ordering.md. Violations (reverse
    /// order, same class) are compile errors, not deadlocks.
    #[inline]
    #[track_caller]
    pub fn lock_nested<'a, Outer: NestKey>(
        &self,
        _outer: &NestProof<'a, Outer>,
    ) -> KeyedGuard<'_, Inner>
    where
        Inner: CanNestInside<Outer>,
    {
        self.lock()
    }
}

/// Zero-sized PROOF that a keyed lock of class `K` is currently held
/// by THIS thread. MINTED ONLY by a live keyed guard's `proof()`
/// (no public constructor); consumed by `lock_nested` /
/// `lock_irqsave_nested`, which only compile when the nesting is
/// permitted. The lifetime ties the proof to the guard that minted
/// it, so it cannot outlive the held section.
pub struct NestProof<'a, K: NestKey> {
    _key: PhantomData<K>,
    _hold: PhantomData<&'a ()>,
}

/// A [`Spinlock<T>`](super::spinlock::Spinlock) whose position in the
/// lock hierarchy is part of its type — the data-carrying twin of
/// [`KeyedRawSpinlock`]. `lock_irqsave` returns the same protocol
/// guard wrapped with the key (call sites unchanged: Deref/DerefMut to
/// the data); the ONLY new capability is `guard.proof()`, which mints
/// the [`NestProof`] other keyed locks require for a nested acquire.
pub struct KeyedSpinlock<K: NestKey, T> {
    inner: super::spinlock::Spinlock<T>,
    _key: PhantomData<K>,
}

// SAFETY: same rule as Spinlock itself — the atomic protocol
// synchronizes; the key is zero-sized and carries no data.
unsafe impl<K: NestKey, T: Send> Sync for KeyedSpinlock<K, T> {}

impl<K: NestKey, T> KeyedSpinlock<K, T> {
    pub const fn new(data: T) -> Self {
        Self { inner: super::spinlock::Spinlock::new(data), _key: PhantomData }
    }

    /// Top-level acquire with the full irqsave protocol (the bucket
    /// locks' historical discipline — unchanged bytes at the call
    /// sites).
    #[inline]
    #[track_caller]
    pub fn lock_irqsave(&self) -> KeyedIrqGuard<'_, K, T> {
        KeyedIrqGuard { inner: self.inner.lock_irqsave(), _key: PhantomData }
    }
}

impl<Inner: NestKey, T> KeyedSpinlock<Inner, T> {
    /// Compile-checked nested acquire — the data-carrying twin of
    /// [`KeyedRawSpinlock::lock_nested`]: only compiles when
    /// `Inner: CanNestInside<Outer>`; same guard (Deref/DerefMut,
    /// proof-minting) as the top-level acquire.
    #[inline]
    #[track_caller]
    pub fn lock_nested<'a, Outer: NestKey>(
        &self,
        _proof: &NestProof<'a, Outer>,
    ) -> KeyedIrqGuard<'_, Inner, T>
    where
        Inner: CanNestInside<Outer>,
    {
        self.lock_irqsave()
    }
}

/// Guard for the data-carrying keyed spinlock: derefs to `T`; mints
/// [`NestProof`] for nested acquisitions while held.
pub struct KeyedIrqGuard<'a, K: NestKey, T: ?Sized> {
    inner: super::spinlock::SpinlockIrqGuard<'a, T>,
    _key: PhantomData<K>,
}

impl<K: NestKey, T: ?Sized> KeyedIrqGuard<'_, K, T> {
    /// Mint the held-section proof (see [`NestProof`]).
    #[inline]
    pub fn proof(&self) -> NestProof<'_, K> {
        NestProof { _key: PhantomData, _hold: PhantomData }
    }
}

impl<K: NestKey, T: ?Sized> core::ops::Deref for KeyedIrqGuard<'_, K, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        self.inner.deref()
    }
}

impl<K: NestKey, T: ?Sized> core::ops::DerefMut for KeyedIrqGuard<'_, K, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        self.inner.deref_mut()
    }
}

/// Guard for a keyed lock — unlocks on drop (raw lock only; the
/// irqsave/preempt layers wrap it from the outside, exactly as
/// `GrqGuard` wraps `RawSpinlock` today).
pub struct KeyedGuard<'a, K: NestKey> {
    lock: *mut KeyedRawSpinlock<K>,
    _key: PhantomData<K>,
    _lifetime: PhantomData<&'a ()>,
}

impl<K: NestKey> Drop for KeyedGuard<'_, K> {
    #[inline]
    fn drop(&mut self) {
        unsafe { (*self.lock).raw.unlock() };
    }
}

impl<K: NestKey> KeyedGuard<'_, K> {
    /// Mint the held-section proof (see [`NestProof`]).
    #[inline]
    pub fn proof(&self) -> NestProof<'_, K> {
        NestProof { _key: PhantomData, _hold: PhantomData }
    }
}

// The guard is the outer-lock proof token; keep it !Send like other
// spinlock guards (it is not `Send` automatically because of the raw
// pointer, and that is the correct default for a hold proof).
