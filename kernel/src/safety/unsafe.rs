//! The safety framework's ONE audited unsafe file (mainline
//! minimal-first consolidation, 2026-11-27): the framework's OWN raw
//! primitives — every `unsafe` block that backs the typed wrappers
//! (bytes, dma, frame, mmio, ptr, ring, slot, call) — live here behind
//! one audited item per primitive family; every framework file is
//! safe code over them. The invariants (INV-KPTR-*, INV-MMIO-*,
//! INV-SLOT-*, INV-RING-*, INV-SAFECALL-*, INV-SAFECAST-*) are
//! documented on the safe wrappers that call these primitives and are
//! restated in condensed form on each edge.
//!
//! Family contract shared by every primitive here: the CALLER (the
//! typed wrapper) has validated pointer validity/alignment/bounds and
//! chosen the invariant that discharges the operation; the primitive
//! performs exactly the raw step and nothing else.

use core::ffi::c_void;
use core::ptr::NonNull;

// ------------------------------------------------------------------
// Byte/slice views (bytes.rs)
// ------------------------------------------------------------------

/// Object bytes as a shared slice (the caller proved size/alignment
/// and the object is live for the borrow).
pub(crate) fn slice_of<'a, T>(v: *const T, n: usize) -> &'a [u8] {
    // SAFETY: caller-validated (live T, n bytes readable).
    unsafe { core::slice::from_raw_parts(v as *const u8, n) }
}

/// Object bytes as an exclusive slice.
pub(crate) fn slice_of_mut<'a, T>(v: *mut T, n: usize) -> &'a mut [u8] {
    // SAFETY: caller-validated (live T, n bytes writable, exclusive).
    unsafe { core::slice::from_raw_parts_mut(v as *mut u8, n) }
}

/// Unaligned read of a Copy value (caller proved the bytes hold a T).
pub(crate) fn read_unaligned<T: Copy>(p: *const T) -> T {
    // SAFETY: caller-validated (initialized T at p, no alignment need).
    unsafe { core::ptr::read_unaligned(p) }
}

/// Unaligned read at a byte offset (caller proved p+off holds a T).
pub(crate) fn read_unaligned_at<T: Copy>(p: *const T, off: usize) -> T {
    // SAFETY: caller-validated (p+off within the object, holds a T).
    unsafe { core::ptr::read_unaligned((p as *const u8).add(off) as *const T) }
}

/// Unaligned write at a byte offset.
pub(crate) fn write_unaligned_at<T: Copy>(p: *mut T, off: usize, val: T) {
    // SAFETY: caller-validated (p+off within the object, writable).
    unsafe { core::ptr::write_unaligned((p as *mut u8).add(off) as *mut T, val) };
}

// ------------------------------------------------------------------
// Volatile access (mmio.rs / dma.rs / ptr.rs / frame.rs)
// ------------------------------------------------------------------

/// Volatile read through a device/DMA window pointer.
pub(crate) fn volatile_read<T: Copy>(p: *const T) -> T {
    // SAFETY: caller-validated (mapped device/DMA window; T: Copy —
    // no partial-drop hazard on a racing write).
    unsafe { core::ptr::read_volatile(p) }
}

/// Volatile read at a byte offset inside a window.
pub(crate) fn volatile_read_at<T: Copy>(p: *const T, off: usize) -> T {
    // SAFETY: caller-validated (p+off inside the window).
    unsafe { core::ptr::read_volatile((p as *const u8).add(off) as *const T) }
}

/// Volatile write through a device/DMA window pointer.
pub(crate) fn volatile_write<T: Copy>(p: *mut T, val: T) {
    // SAFETY: caller-validated (mapped, writable window).
    unsafe { core::ptr::write_volatile(p, val) }
}

/// Volatile write at a byte offset inside a window.
pub(crate) fn volatile_write_at<T: Copy>(p: *mut T, off: usize, val: T) {
    // SAFETY: caller-validated (p+off inside the window).
    unsafe { core::ptr::write_volatile((p as *mut u8).add(off) as *mut T, val) }
}

// ------------------------------------------------------------------
// Raw allocation (dma.rs / ptr.rs / KPtr Drop)
// ------------------------------------------------------------------

/// Allocate a zero-sized-check'd layout (caller built the Layout and
/// checks the null return).
pub(crate) fn alloc_layout(layout: core::alloc::Layout) -> *mut u8 {
    // SAFETY: caller-validated (non-zero-size layout; null-checked).
    unsafe { alloc::alloc::alloc(layout) }
}

/// Free with the exact construction layout (INV-KPTR-3: same T at both
/// ends; drop glue already run by the caller).
pub(crate) fn dealloc(ptr: *mut u8, layout: core::alloc::Layout) {
    // SAFETY: caller-validated (matching construction layout).
    unsafe { alloc::alloc::dealloc(ptr, layout) }
}

/// Run T's drop glue in place (INV-KPTR-3: live, uniquely owned).
pub(crate) fn drop_in_place<T>(ptr: *mut T) {
    // SAFETY: caller-validated (live unique T, drop glue not yet run).
    unsafe { core::ptr::drop_in_place(ptr) }
}

/// In-object offset (caller bounds-checked).
pub(crate) fn ptr_add<T>(p: *const T, off: usize) -> *const T {
    // SAFETY: caller-validated (p+off within the object/window).
    unsafe { (p as *const u8).add(off) as *const T }
}

/// In-object mutable offset (caller bounds-checked).
pub(crate) fn ptr_add_mut<T>(p: *mut T, off: usize) -> *mut T {
    // SAFETY: caller-validated (p+off within the object/window).
    unsafe { (p as *mut u8).add(off) as *mut T }
}

// ------------------------------------------------------------------
// NonNull views (dma.rs / ptr.rs)
// ------------------------------------------------------------------

/// Shared view behind a NonNull (caller proves the pointee is
/// initialized and the borrow is valid for the lifetime).
pub(crate) fn nonnull_ref<'a, T: ?Sized>(ptr: NonNull<T>) -> &'a T {
    // SAFETY: caller-validated (live, initialized pointee).
    unsafe { ptr.as_ref() }
}

/// Exclusive view behind a NonNull.
pub(crate) fn nonnull_mut<'a, T: ?Sized>(ptr: NonNull<T>) -> &'a mut T {
    // SAFETY: caller-validated (&mut provenance proves exclusivity);
    // as_mut on a raw copy of the NonNull (shared, not borrowed).
    unsafe { (&mut { ptr }).as_mut() }
}

/// NonNull from a non-null raw pointer.
pub(crate) const fn nonnull_new<T: ?Sized>(ptr: *const T) -> NonNull<T> {
    // SAFETY: caller-validated (ptr non-null).
    unsafe { NonNull::new_unchecked(ptr as *mut T) }
}

/// Write through a fresh NonNull allocation (KPtr::new; the allocation
/// is uninitialized until this store).
pub(crate) fn nonnull_write<T>(ptr: NonNull<T>, value: T) {
    // SAFETY: caller-validated (fresh allocation, exactly size_of::<T>()).
    unsafe { ptr.as_ptr().write(value) };
}

// ------------------------------------------------------------------
// Cast edge (call.rs)
// ------------------------------------------------------------------

/// The `transmute_copy` behind `safety_cast` (INV-SAFECAST-1: the
/// caller asserted same-size and Copy before calling).
pub(crate) fn transmute_copy<Src: Copy, Dst: Copy>(src: &Src) -> Dst {
    // SAFETY: INV-SAFECAST-1 — sizes asserted equal by the caller;
    // both Copy (no drop glue skipped).
    unsafe { core::mem::transmute_copy(src) }
}

/// The `safe_call!` macro — generate a safe wrapper around one
/// `unsafe fn` (INV-SAFECALL-1/2/3). The macro BODY carries the unsafe
/// call, so it lives in this audited file; `#[macro_export]` keeps it
/// at the crate root for every existing call site.
#[macro_export]
macro_rules! safe_call {
    ($(#[$m:meta])* $vis:vis fn $name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty = unsafe $callee:expr $(;)?) => {
        $(#[$m])*
        #[inline(always)]
        $vis fn $name($($arg: $ty),*) -> $ret {
            // SAFETY: INV-SAFECALL-1 — this wrapper's doc names the
            // discharged invariant; the types carry the proof
            // (INV-SAFECALL-2).
            $callee($($arg),*)
        }
    };
    ($(#[$m:meta])* $vis:vis fn $name:ident($($arg:ident: $ty:ty),* $(,)?) = unsafe $callee:expr $(;)?) => {
        $crate::safe_call! {
            $(#[$m])*
            $vis fn $name($($arg: $ty),*) -> () = unsafe $callee;
        }
    };
}

// ------------------------------------------------------------------
// Send/Sync commitments (moved verbatim with their SAFETY notes)
// ------------------------------------------------------------------

// SAFETY: KPtr owns its allocation exclusively (INV-KPTR-1/2); Send
// moves the unique ownership, Sync never shares &mut.
unsafe impl<T: Send> Send for super::ptr::KPtr<T> {}
unsafe impl<T: Sync> Sync for super::ptr::KPtr<T> {}

// SAFETY: SpscRing's indices are atomics and the slot hand-off is the
// documented producer/consumer protocol (INV-RING-1/3).
unsafe impl<T: Copy + Send, const N: usize> Send for super::ring::SpscRing<T, N> {}
unsafe impl<T: Copy + Send, const N: usize> Sync for super::ring::SpscRing<T, N> {}

// SAFETY: OnceSlot publishes once (INV-SLOT-1/2); readers see the
// value only after the Release flag.
unsafe impl<T: Send + Sync> Sync for super::slot::OnceSlot<T> {}
unsafe impl<T: Send> Send for super::slot::OnceSlot<T> {}

// ------------------------------------------------------------------
// Misc: c_void import kept for the DMA helpers' raw shapes
// ------------------------------------------------------------------

/// The c_void alias the DMA descriptor views name (kept here so the
/// framework files stay import-clean).
pub(crate) type Void = c_void;

// ------------------------------------------------------------------
// Slot/ring cell primitives
// ------------------------------------------------------------------

/// Write into a not-yet-published MaybeUninit cell (INV-SLOT-1: no
/// reader can be reading — readers only read a set flag).
pub(crate) fn slot_write<T>(cell: *mut core::mem::MaybeUninit<T>, make: impl FnOnce() -> T) {
    // SAFETY: INV-SLOT-1 — unpublished cell, single writer.
    unsafe { (*cell).write(make()); }
}

/// Shared view of a published cell (INV-SLOT-1/2: the flag was
/// observed set, so the value was written before the Acquire).
pub(crate) fn slot_ref<'a, T>(cell: *const core::mem::MaybeUninit<T>) -> &'a T {
    // SAFETY: INV-SLOT-1/2 — published, never overwritten or dropped.
    unsafe { (*cell).assume_init_ref() }
}

/// Exclusive view of a published cell (INV-SLOT-1/2).
pub(crate) fn slot_mut<'a, T>(cell: *mut core::mem::MaybeUninit<T>) -> &'a mut T {
    // SAFETY: INV-SLOT-1/2 — published; &mut self proves exclusivity.
    unsafe { (*cell).assume_init_mut() }
}

/// Producer slot store (INV-RING-1: idx within 0..N by the mask
/// discipline; the slot is owned by the single producer until `tail`
/// publishes it; MaybeUninit store is a plain Copy store).
pub(crate) fn ring_slot_write<T: Copy, const N: usize>(
    slots: &core::cell::UnsafeCell<[core::cell::UnsafeCell<core::mem::MaybeUninit<T>>; N]>,
    idx: usize,
    val: T,
) {
    // SAFETY: INV-RING-1 — see above; the slots cell is the ring's
    // machine-lifetime storage.
    unsafe {
        (*slots.get())[idx].get().write(core::mem::MaybeUninit::new(val));
    }
}

/// Consumer slot read (INV-RING-1/3: idx within 0..N; the Acquire on
/// tail ordered the producer's store before this read; volatile read,
/// T: Copy, no drop).
pub(crate) fn ring_slot_read<T: Copy, const N: usize>(
    slots: &core::cell::UnsafeCell<[core::cell::UnsafeCell<core::mem::MaybeUninit<T>>; N]>,
    idx: usize,
) -> T {
    // SAFETY: INV-RING-1/3 — see above.
    unsafe {
        let slot_ptr = (*slots.get())[idx].get() as *const core::mem::MaybeUninit<T>;
        core::ptr::read_volatile(slot_ptr as *const T)
    }
}
