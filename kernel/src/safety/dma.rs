#![forbid(unsafe_code)]
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
    /// MPK phase 1: the byte-offset range whose pages carry pkey=1
    /// (None = unguarded block). Accesses inside the range open the
    /// key-1 zone (WRPKRU save/restore) around the volatile op —
    /// everything else on the block is untouched. The range is set
    /// ONCE at vring creation, before the queue goes live.
    pkey_range: Option<(usize, usize)>,
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
        let raw = super::edges::alloc_layout(layout);
        Some(DmaBlock {
            ptr: NonNull::new(raw)?,
            len,
            layout,
            frontier: 0,
            pkey_range: None,
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
        let base = super::edges::ptr_add_mut(self.ptr.as_ptr(), off) as *mut T;
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
        let base = super::edges::ptr_add_mut(self.ptr.as_ptr(), off) as *mut T;
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
        // MPK phase 1: accesses into the pkey-tagged range open the
        // zone (save/restore WRPKRU — IRQ-nesting-safe). Off-range
        // (and guard-off) accesses pay one Option check.
        if let Some((lo, hi)) = self.pkey_range {
            if off >= lo && off < hi && crate::arch::mpk::guard_on() {
                let prev = crate::arch::mpk::zone_open();
                let v = self.read_volatile_at_inner::<T>(off);
                crate::arch::mpk::zone_close(prev);
                return v;
            }
        }
        self.read_volatile_at_inner::<T>(off)
    }

    fn read_volatile_at_inner<T: Copy>(&self, off: usize) -> T {
        // SAFETY: INV-DMA-1/3 — live allocation, bounds-checked
        // offset, volatile because the device writes concurrently;
        // T is Copy so no partial-drop hazard.
        super::edges::volatile_read_at(self.ptr.as_ptr() as *const T, off)
    }

    /// Bounds-checked volatile write (INV-DMA-3 mirror).
    pub fn write_volatile_at<T: Copy>(&self, off: usize, val: T) {
        let end = off.checked_add(core::mem::size_of::<T>());
        match end {
            Some(e) if e <= self.len => {}
            _ => panic!("DMA write out of block: off={:#x} len={:#x}", off, self.len),
        }
        // MPK phase 1: as the read twin's zone window.
        if let Some((lo, hi)) = self.pkey_range {
            if off >= lo && off < hi && crate::arch::mpk::guard_on() {
                let prev = crate::arch::mpk::zone_open();
                self.write_volatile_at_inner::<T>(off, val);
                crate::arch::mpk::zone_close(prev);
                return;
            }
        }
        self.write_volatile_at_inner::<T>(off, val)
    }

    fn write_volatile_at_inner<T: Copy>(&self, off: usize, val: T) {
        // SAFETY: as read_volatile_at, write direction.
        super::edges::volatile_write_at(self.ptr.as_ptr() as *mut T, off, val)
    }

    /// Declare the pkey-protected byte range (MPK phase 1) and tag its
    /// pages. Call ONCE at creation time, before the block goes live;
    /// `[lo, hi)` are offsets within the block. Returns the pages
    /// tagged (0 = the guard is off — the range is not stored, the
    /// block behaves unguarded).
    pub fn set_pkey_range(&mut self, lo: usize, hi: usize) -> usize {
        if !crate::arch::mpk::guard_on() {
            return 0;
        }
        if lo >= hi || hi > self.len {
            return 0;
        }
        let base = self.ptr.as_ptr() as usize;
        let tagged = crate::arch::mpk::tag_kernel_pages(base + lo, hi - lo);
        if tagged > 0 {
            self.pkey_range = Some((lo, hi));
        }
        tagged
    }

    /// Clear the pkey tag on the block's pages and forget the range
    /// (the pre-Drop companion: a freed tagged page would re-enter
    /// the allocator STILL carrying pkey=1 — every later user of that
    /// page faults. Call from the owning structure's teardown BEFORE
    /// the block drops).
    pub fn clear_pkey_range(&mut self) {
        if let Some((lo, hi)) = self.pkey_range.take() {
            let base = self.ptr.as_ptr() as usize;
            crate::arch::mpk::untag_kernel_pages(base + lo, hi - lo);
        }
    }

    /// The guarded range's first VA (the escape probe's target), or
    /// None when unguarded.
    pub fn pkey_target(&self) -> Option<*mut u64> {
        self.pkey_range
            .map(|(lo, _)| super::edges::ptr_add_mut(self.ptr.as_ptr(), lo) as *mut u64)
    }
}

impl Drop for DmaBlock {
    fn drop(&mut self) {
        // SAFETY: INV-DMA-1 — dealloc with the exact layout stored at
        // construction (never mutated).
        super::edges::dealloc(self.ptr.as_ptr(), self.layout)
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
        super::edges::volatile_read(self.ptr.as_ptr() as *const T)
    }

    /// Volatile write of the whole slot value (INV-DMA-3 mirror).
    pub fn write_volatile(&self, val: T)
    where
        T: Copy,
    {
        // SAFETY: as read_volatile; the slot's range is owned by this
        // slot (INV-DMA-2), volatility is for the device reader.
        super::edges::volatile_write(self.ptr.as_ptr() as *mut T, val)
    }
}

impl<T: ?Sized> Deref for DmaSlot<T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: INV-DMA-2 — the slot's range is live for the owning
        // block's lifetime and disjoint from every other slot.
        super::edges::nonnull_ref(self.ptr)
    }
}

impl<T: ?Sized> DerefMut for DmaSlot<T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: INV-DMA-2 — &mut self proves this slot's exclusive
        // access within its disjoint range.
        super::edges::nonnull_mut(self.ptr)
    }
}
