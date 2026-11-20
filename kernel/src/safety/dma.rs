//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Device-shared memory blocks — the virtqueue/DMA family's primitive
//! (unsafe-slim round 3 lane A).
//!
//! A virtqueue is ONE allocation carved into three disjoint
//! sub-objects (descriptor table, available ring, used ring) that the
//! device reads and writes concurrently. The old code held three raw
//! pointers derived by hand arithmetic from one `alloc` result, with
//! per-use unsafe derefs and volatile index reads scattered through
//! the hot path. `DmaBlock` performs the carve ONCE with alignment,
//! size, and disjointness checks; `DmaSlot` is the typed view.
//!
//! Storage shape: the owning struct (e.g. `VirtQueue`) stores the
//! `DmaBlock` and the slots side by side. Slots are zero-sized views
//! with no drop, so struct drop order is irrelevant; the block is the
//! single owner that deallocates. The "block outlives slots" rule is
//! INV-DMA-2's second half and holds structurally because both live
//! in the same struct.

use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::ptr::NonNull;

/// A kernel allocation shared with a device, carved into disjoint
/// typed slots.
///
/// Invariants:
///
/// * **INV-DMA-1 (allocation)** — one contiguous `alloc` result
///   (null-checked at `new`), `len` bytes; `Drop` deallocates with
///   the SAME layout, stored at construction and never mutated.
/// * **INV-DMA-2 (carve)** — `carve*` checks alignment to
///   `align_of::<T>()`, bounds within `len`, and DISJOINTNESS from
///   previously carved ranges (append-only frontier). The owning
///   `DmaBlock` must outlive every slot carved from it — hold both
///   in the same struct (slots are zero-drop, so field order does
///   not matter).
/// * **INV-DMA-3 (device concurrency)** — driver-owned fields may use
///   normal `Deref`/`DerefMut`; fields the DEVICE writes concurrently
///   must be read via `read_volatile*` paths (bounds-checked), never
///   plain `Deref`, so the access is neither torn nor elided.
pub struct DmaBlock {
    ptr: NonNull<u8>,
    len: usize,
    layout: core::alloc::Layout,
    frontier: usize,
}

impl DmaBlock {
    /// The audited allocation edge (INV-DMA-1).
    pub fn new(len: usize, align: usize) -> Option<Self> {
        if len == 0 {
            return None;
        }
        let layout = core::alloc::Layout::from_size_align(len, align).ok()?;
        // SAFETY: INV-DMA-1 — non-zero size layout; the raw block is
        // adopted exclusively by this DmaBlock; null-checked below.
        let raw = unsafe { alloc::alloc::alloc(layout) };
        Some(DmaBlock {
            ptr: NonNull::new(raw)?,
            len,
            layout,
            frontier: 0,
        })
    }

    /// Contiguous physical address of the block (for the device's
    /// queue registers). The block must stay alive while programmed.
    pub fn phys(&self) -> u64 {
        crate::arch::mm::virt_to_phys(crate::arch::mm::VirtAddr::new(
            self.ptr.as_ptr() as u64,
        ))
        .0
    }

    /// Virtual address of the block (register programming that takes
    /// the VA, or legacy VA-based paths).
    pub fn addr(&self) -> usize {
        self.ptr.as_ptr() as usize
    }

    /// Physical address of byte offset `off` (a previously returned
    /// carve offset; unchecked arithmetic on purpose — offsets come
    /// from carve results, not device input).
    pub fn phys_at(&self, off: usize) -> u64 {
        crate::arch::mm::virt_to_phys(crate::arch::mm::VirtAddr::new(
            self.ptr.as_ptr() as u64 + off as u64,
        ))
        .0
    }

    /// Carve `count` contiguous elements of `T` at the frontier
    /// (INV-DMA-2: aligned, bounds-checked, disjoint from earlier
    /// carves). Returns the typed slot and its byte offset (the
    /// offset is what queue registers get via `phys_at`).
    pub fn carve_array<T>(&mut self, count: usize) -> Option<(DmaSlot<[T]>, usize)> {
        let size = core::mem::size_of::<T>().checked_mul(count)?;
        let align = core::mem::align_of::<T>().max(1);
        let off = (self.frontier + align - 1) & !(align - 1);
        let end = off.checked_add(size)?;
        if end > self.len {
            return None;
        }
        self.frontier = end;
        // SAFETY: INV-DMA-2 — off+size bounds-checked above; off is
        // aligned; [off, end) is disjoint from all earlier carves
        // because the frontier only advances.
        let base = unsafe { self.ptr.as_ptr().add(off) } as *mut T;
        let ptr = NonNull::slice_from_raw_parts(NonNull::new(base)?, count);
        Some((DmaSlot { ptr, _tag: PhantomData }, off))
    }

    /// Carve `count` elements of `T` at the frontier with an EXPLICIT
    /// alignment (stricter than `align_of::<T>`), for layouts the
    /// device protocol fixes (e.g. page-aligned vring sections).
    /// Same INV-DMA-2 checks as [`carve_array`].
    pub fn carve_array_aligned<T>(
        &mut self,
        count: usize,
        align: usize,
    ) -> Option<(DmaSlot<[T]>, usize)> {
        let size = core::mem::size_of::<T>().checked_mul(count)?;
        let align = align.max(1);
        let off = (self.frontier + align - 1) & !(align - 1);
        let end = off.checked_add(size)?;
        if end > self.len {
            return None;
        }
        self.frontier = end;
        // SAFETY: INV-DMA-2 — bounds/alignment/disjointness proven
        // above (explicit align variant).
        let base = unsafe { self.ptr.as_ptr().add(off) } as *mut T;
        let ptr = NonNull::slice_from_raw_parts(NonNull::new(base)?, count);
        Some((DmaSlot { ptr, _tag: PhantomData }, off))
    }

    /// Carve ONE `T` at the frontier with an EXPLICIT alignment
    /// (device-protocol-fixed layouts); same INV-DMA-2 checks.
    pub fn carve_aligned<T>(&mut self, align: usize) -> Option<(DmaSlot<T>, usize)> {
        let (slot, off) = self.carve_array_aligned::<T>(1, align)?;
        // SAFETY: 1-element slice; base pointer is a valid aligned *mut T.
        let ptr = NonNull::new(slot.ptr.as_ptr() as *mut T)?;
        Some((DmaSlot { ptr, _tag: PhantomData }, off))
    }

    /// Carve ONE `T` at the frontier (INV-DMA-2).
    pub fn carve<T>(&mut self) -> Option<(DmaSlot<T>, usize)> {
        let (slot, off) = self.carve_array::<T>(1)?;
        // SAFETY: carve_array(1) returned a 1-element slice; the base
        // pointer is a valid *mut T with the same alignment/bounds
        // proof.
        let ptr = NonNull::new(slot.ptr.as_ptr() as *mut T)?;
        Some((DmaSlot { ptr, _tag: PhantomData }, off))
    }

    /// Byte offset of the carve frontier (for size assertions).
    pub fn frontier_offset(&self) -> usize {
        self.frontier
    }

    /// Block length in bytes.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Bounds-checked volatile read of a `T` at byte offset `off`
    /// (INV-DMA-3): for device-written fields (ring indices, response
    /// status bytes). Any in-block `off` is sound; a wrong `off`
    /// panics loudly instead of walking foreign memory.
    pub fn read_volatile_at<T: Copy>(&self, off: usize) -> T {
        let end = off.checked_add(core::mem::size_of::<T>());
        match end {
            Some(e) if e <= self.len => {}
            _ => panic!("DMA read out of block: off={:#x} len={:#x}", off, self.len),
        }
        // SAFETY: INV-DMA-1/3 — live allocation, bounds-checked
        // offset, volatile because the device writes concurrently;
        // T is Copy so no partial-drop hazard.
        unsafe { core::ptr::read_volatile(self.ptr.as_ptr().add(off) as *const T) }
    }

    /// Bounds-checked volatile write (INV-DMA-3 mirror).
    pub fn write_volatile_at<T: Copy>(&self, off: usize, val: T) {
        let end = off.checked_add(core::mem::size_of::<T>());
        match end {
            Some(e) if e <= self.len => {}
            _ => panic!("DMA write out of block: off={:#x} len={:#x}", off, self.len),
        }
        // SAFETY: as read_volatile_at, write direction.
        unsafe { core::ptr::write_volatile(self.ptr.as_ptr().add(off) as *mut T, val) }
    }
}

impl Drop for DmaBlock {
    fn drop(&mut self) {
        // SAFETY: INV-DMA-1 — dealloc with the exact layout stored at
        // construction (never mutated).
        unsafe { alloc::alloc::dealloc(self.ptr.as_ptr(), self.layout) }
    }
}

/// A typed, disjoint view into a [`DmaBlock`] (INV-DMA-2).
///
/// `Deref`/`DerefMut` are safe for driver-owned fields. Fields the
/// device writes concurrently must be read via `read_volatile`
/// (INV-DMA-3). The owning block must outlive the slot (store both in
/// the same struct — see the module doc).
pub struct DmaSlot<T: ?Sized> {
    ptr: NonNull<T>,
    _tag: PhantomData<*mut T>,
}

impl<T: ?Sized> DmaSlot<T> {
    /// Raw address (escape hatch for descriptor programming; the
    /// block-lifetime rule still applies).
    /// (INV-DMA-2 hatch row: window-lifetime address, no ownership.)
    pub fn as_ptr(&self) -> *mut T {
        self.ptr.as_ptr()
    }

    /// Physical address of this slot (DMA register programming).
    pub fn phys(&self) -> u64 {
        crate::arch::mm::virt_to_phys(crate::arch::mm::VirtAddr::new(
            self.ptr.as_ptr() as *mut u8 as u64,
        ))
        .0
    }
}

impl<T> DmaSlot<T> {
    /// Volatile read of the whole slot value (INV-DMA-3): for
    /// device-written slots (response buffers, ring headers).
    pub fn read_volatile(&self) -> T
    where
        T: Copy,
    {
        // SAFETY: INV-DMA-2 — live, disjoint, correctly-typed range;
        // volatile for device concurrency; T: Copy.
        unsafe { core::ptr::read_volatile(self.ptr.as_ptr()) }
    }

    /// Volatile write of the whole slot value (INV-DMA-3 mirror).
    pub fn write_volatile(&self, val: T)
    where
        T: Copy,
    {
        // SAFETY: as read_volatile; the slot's range is owned by this
        // slot (INV-DMA-2), volatility is for the device reader.
        unsafe { core::ptr::write_volatile(self.ptr.as_ptr(), val) }
    }
}

impl<T: ?Sized> Deref for DmaSlot<T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: INV-DMA-2 — the slot's range is live for the owning
        // block's lifetime and disjoint from every other slot.
        unsafe { self.ptr.as_ref() }
    }
}

impl<T: ?Sized> DerefMut for DmaSlot<T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: INV-DMA-2 — &mut self proves this slot's exclusive
        // access within its disjoint range.
        unsafe { self.ptr.as_mut() }
    }
}
