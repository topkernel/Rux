//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Owned and shared typed kernel pointers — the raw-deref family's
//! primitives (unsafe-slim round 3 lane A).
//!
//! Before this module, a long-lived heap object (a virtio request
//! header, a DMA response slot, a carved ring) lived as a bare
//! `*mut T` from `alloc::alloc::alloc` to `dealloc`, with one
//! `unsafe { (*p).field }` per access and one hand-written
//! `dealloc(ptr, layout)` per exit path — the layout-drift and
//! double-free bug classes lived at every exit. `KPtr` compresses
//! that to ONE audited construction and ONE audited drop; access
//! in between is safe.

use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;

/// Owned kernel allocation — the `Box` discipline for memory that the
/// kernel (and sometimes a device, via the raw address) shares.
///
/// Invariants (Kani-friendly; see `kernel/verify`):
///
/// * **INV-KPTR-1 (construction)** — a `KPtr` is non-null, aligned to
///   `align_of::<T>()`, and points at an initialized `T`. The ONE
///   audited allocation is `KPtr::new` (null-checked, layout derived
///   from the same `T` as the eventual dealloc). All other
///   constructors are `unsafe` edges whose proof is per-site.
/// * **INV-KPTR-2 (uniqueness)** — exactly one `KPtr` owns the
///   allocation (same aliasing contract as `Box<T>`); therefore
///   `Deref`/`DerefMut` are safe and mutation needs no lock.
/// * **INV-KPTR-3 (drop)** — `Drop` deallocates with the same
///   `Layout::new::<T>()` the value was allocated with. The layout
///   cannot drift because both ends derive from the one type
///   parameter. Dropping a leaked-then-re-adopted pointer is
///   prevented by construction (see `from_raw`).
/// * **INV-KPTR-4 (raw escape)** — `as_ptr`/`phys` hand out the raw
///   address for device DMA. The `KPtr` must stay alive while the
///   device may write; the address must not be freed by anyone else.
///   `into_raw`/`from_raw` transfer ownership; `from_raw` is the
///   audited re-adoption edge.
/// * **INV-KPTR-5 (device-written memory)** — when the device writes
///   the allocation concurrently, reads must go through
///   `read_volatile` (provided), never `Deref`, so the access cannot
///   be torn or elided by the compiler.
pub struct KPtr<T> {
    ptr: NonNull<T>,
}

impl<T> KPtr<T> {
    /// Allocate and place `value` — the ONE safe construction.
    ///
    /// Returns `None` on allocation failure (callers map to -ENOMEM);
    /// the kernel's `#[alloc_error_handler]` is not invoked here.
    pub fn new(value: T) -> Option<Self> {
        let layout = core::alloc::Layout::new::<T>();
        if layout.size() == 0 {
            // ZST: no allocation, dangling-but-aligned is the canonical
            // ZST reference (INV-KPTR-1: an initialized T exists — the
            // one in `value`).
            return Some(KPtr { ptr: NonNull::dangling() });
        }
        // SAFETY: INV-KPTR-1 — layout is non-zero (checked above) and
        // derived from T; the raw block is immediately ownedexclusively
        // by the KPtr we return exclusively; null is checked before adoption.
        let raw = unsafe { alloc::alloc::alloc(layout) } as *mut T;
        let ptr = NonNull::new(raw)?;
        // SAFETY: INV-KPTR-1 — ptr is valid for writes of T (fresh
        // allocation of T's layout) and not aliased.
        unsafe { ptr.as_ptr().write(value) };
        Some(KPtr { ptr })
    }

    /// Raw address of the allocation (INV-KPTR-4 escape hatch: for
    /// DMA descriptor programming; the KPtr stays alive).
    pub fn as_ptr(&self) -> *mut T {
        self.ptr.as_ptr()
    }

    /// Physical address of the allocation for device DMA
    /// (INV-KPTR-4). Same obligation as `as_ptr`.
    pub fn phys(&self) -> u64 {
        crate::arch::mm::virt_to_phys(crate::arch::mm::VirtAddr::new(
            self.ptr.as_ptr() as u64,
        ))
        .0
    }

    /// Volatile read of the whole value (INV-KPTR-5): the device may
    /// be writing this memory concurrently with the read.
    pub fn read_volatile(&self) -> T
    where
        T: Copy,
    {
        // SAFETY: INV-KPTR-1/2 — the pointer is a live, correctly
        // typed allocation; volatile because of concurrent device
        // writes (INV-KPTR-5). T is Copy so no partial-drop hazard.
        unsafe { core::ptr::read_volatile(self.ptr.as_ptr()) }
    }

    /// Volatile write of the whole value (INV-KPTR-5 mirror).
    pub fn write_volatile(&self, value: T)
    where
        T: Copy,
    {
        // SAFETY: same as read_volatile; the KPtr owns the slot
        // exclusively (INV-KPTR-2), the volatility is for the device
        // reader on the other end.
        unsafe { core::ptr::write_volatile(self.ptr.as_ptr(), value) }
    }

    /// Surrender ownership without dropping (INV-KPTR-4): for
    /// ownership transfers through raw land (in-flight DMA tables).
    /// The caller must re-adopt via `from_raw` exactly once or leak.
    pub fn into_raw(self) -> NonNull<T> {
        let ptr = self.ptr;
        core::mem::forget(self);
        ptr
    }

    /// Re-adopt a pointer previously released by `into_raw` (the
    /// audited edge: the caller proves the pointer is the same live
    /// allocation and that ownership really transferred back).
    ///
    /// # Safety
    /// `ptr` must originate from `KPtr::into_raw` (or `new`'s raw
    /// equivalent) for a still-live allocation of `T`, and must not
    /// be adopted twice.
    pub unsafe fn from_raw(ptr: NonNull<T>) -> Self {
        KPtr { ptr }
    }
}

impl<T> Deref for KPtr<T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: INV-KPTR-1/2 — live, uniquely-owned, initialized T.
        // For device-written fields use read_volatile (INV-KPTR-5).
        unsafe { self.ptr.as_ref() }
    }
}

impl<T> DerefMut for KPtr<T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: INV-KPTR-2 — &mut self proves no other reference to
        // this allocation exists right now.
        unsafe { self.ptr.as_mut() }
    }
}

impl<T> Drop for KPtr<T> {
    fn drop(&mut self) {
        if core::mem::size_of::<T>() == 0 {
            return; // ZST: nothing was allocated (INV-KPTR-3).
        }
        // SAFETY: INV-KPTR-3 — the layout is the same Layout::new::<T>()
        // used at construction (same T, both ends); run T's drop glue
        // first (as_ref_mut on a live unique allocation).
        unsafe {
            core::ptr::drop_in_place(self.ptr.as_ptr());
            alloc::alloc::dealloc(
                self.ptr.as_ptr() as *mut u8,
                core::alloc::Layout::new::<T>(),
            );
        }
    }
}

// KPtr moves between CPUs (task context, IRQ handlers); the pointee's
// own synchronization (or unique ownership) governs access, exactly as
// it did for the raw pointers this replaces.
unsafe impl<T: Send> Send for KPtr<T> {}
unsafe impl<T: Sync> Sync for KPtr<T> {}

/// Shared reference to machine-lifetime storage — the
/// "registered static / boxed-once / deliberately leaked" family
/// (the INV-BLKDEV-1 pattern generalized).
///
/// Invariants:
///
/// * **INV-SREF-1 (storage)** — the target lives in storage that
///   outlives every reference: a `static`, a registry entry boxed
///   once and never removed, or a deliberate leak. Each
///   `from_parts` site names ITS storage in a SAFETY comment; that
///   is the one audited edge per site.
/// * **INV-SREF-2 (sharedness)** — `StaticRef` never hands out
///   `&mut`. Legitimate mutation of the target stays behind the
///   storage's own lock (e.g. the registry Spinlock), never through
///   this type.
/// * **INV-SREF-3 (provenance)** — the pointer was derived from a
///   live reference to the same storage; no offset arithmetic
///   inside `StaticRef` (carve sub-objects with `map`, which re-audits).
pub struct StaticRef<T: ?Sized + 'static> {
    ptr: NonNull<T>,
    _lt: PhantomData<&'static T>,
}

impl<T: ?Sized + 'static> StaticRef<T> {
    /// Adopt a `&'static T` — construction is safe because the
    /// reference IS the proof (INV-SREF-1).
    pub const fn from_static(r: &'static T) -> Self {
        // SAFETY: a &'static T is non-null and valid for 'static;
        // const-constructible so registry tables can hold StaticRefs.
        let ptr = r as *const T as *mut T;
        // SAFETY: non-null by construction from a reference.
        StaticRef { ptr: unsafe { NonNull::new_unchecked(ptr) }, _lt: PhantomData }
    }

    /// Adopt a raw pointer (the audited per-site edge). Each call
    /// site's SAFETY comment names the storage that outlives the
    /// machine (INV-SREF-1) and the provenance (INV-SREF-3).
    ///
    /// # Safety
    /// `ptr` must point at a `T` that is valid for the machine's
    /// remaining lifetime and never be freed or mutably aliased
    /// except through its storage's own lock.
    pub unsafe fn from_parts(ptr: *const T) -> Option<Self> {
        NonNull::new(ptr as *mut T).map(|ptr| StaticRef { ptr, _lt: PhantomData })
    }

    /// The machine-lifetime shared reference (INV-SREF-1/2).
    pub fn get(&self) -> &'static T {
        // SAFETY: INV-SREF-1 — storage outlives the machine, so the
        // borrow can be 'static; shared because StaticRef hands out
        // no &mut (INV-SREF-2).
        unsafe { self.ptr.as_ref() }
    }

    /// Re-audit a sub-object of the storage as another StaticRef
    /// (INV-SREF-3): the projection must itself satisfy INV-SREF-1/2.
    pub fn map<U: ?Sized + 'static, F: FnOnce(&T) -> &U>(self, f: F) -> StaticRef<U> {
        let r: &'static U = f(self.get());
        StaticRef { ptr: NonNull::from(r), _lt: PhantomData }
    }
}

impl<T: ?Sized + 'static> Deref for StaticRef<T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.get()
    }
}

impl<T: ?Sized + 'static> Clone for StaticRef<T> {
    fn clone(&self) -> Self {
        StaticRef { ptr: self.ptr, _lt: PhantomData }
    }
}

impl<T: ?Sized + 'static> Copy for StaticRef<T> {}
