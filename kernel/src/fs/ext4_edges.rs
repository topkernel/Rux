//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The ext4 TYPED-VIEW FLOOR (rung 5): the on-disk struct views the
//! ext4 service crate consumes, as audited framework edges — the
//! crate-side files ride these under `#![forbid(unsafe_code)]`.
//!
//! INV-EXT-EDGE-1 (bounds + alignment): every view checks BOTH the
//! byte span and the target alignment before the cast. The alignment
//! check makes the edge sound on its own terms — the pre-rung code
//! relied on the block-cache allocator's de-facto alignment for
//! `Vec<u8>` block buffers; a hypothetical misaligned buffer now
//! yields the empty/None arm (the corrupt-disk class), never UB.
//!
//! INV-EXT-EDGE-2 (u32-array header views): a `&[u32; N]` array IS
//! 4-aligned, so a `#[repr(C)]` header/entry type whose alignment is
//! ≤ 4 may view into it — the i_block family (the cached Ext4Inode
//! `block` field and the on-disk `i_block` share the layout).

/// Typed `&T` view of `data`'s start: size + alignment checked; None
/// when the span is too small or misaligned (INV-EXT-EDGE-1).
pub fn typed_ref<T: Copy>(data: &[u8]) -> Option<&T> {
    let aligned = (data.as_ptr() as usize) % core::mem::align_of::<T>() == 0;
    if data.len() < core::mem::size_of::<T>() || !aligned {
        return None;
    }
    // SAFETY: INV-EXT-EDGE-1 — span and alignment checked above.
    Some(unsafe { &*(data.as_ptr() as *const T) })
}

/// Exclusive twin of [`typed_ref`].
pub fn typed_ref_mut<T: Copy>(data: &mut [u8]) -> Option<&mut T> {
    let aligned = (data.as_ptr() as usize) % core::mem::align_of::<T>() == 0;
    if data.len() < core::mem::size_of::<T>() || !aligned {
        return None;
    }
    // SAFETY: INV-EXT-EDGE-1 — span and alignment checked; the &mut
    // window rides the caller's exclusive slice.
    Some(unsafe { &mut *(data.as_mut_ptr() as *mut T) })
}

/// Typed `&[T]` view of `count` entries at byte offset `off`:
/// bounds + alignment checked; the EMPTY slice when the span does not
/// fit or is misaligned (INV-EXT-EDGE-1).
pub fn typed_slice<T: Copy>(data: &[u8], off: usize, count: usize) -> &[T] {
    let need = match count.checked_mul(core::mem::size_of::<T>()) {
        Some(cn) if count > 0 => match off.checked_add(cn) {
            Some(n) => n,
            None => return &[],
        },
        _ => return &[],
    };
    if data.len() < need {
        return &[];
    }
    let p = data.as_ptr().wrapping_add(off);
    if (p as usize) % core::mem::align_of::<T>() != 0 {
        return &[];
    }
    // SAFETY: INV-EXT-EDGE-1 — bounds and alignment checked above.
    unsafe { core::slice::from_raw_parts(p as *const T, count) }
}

/// Exclusive twin of [`typed_slice`].
pub fn typed_slice_mut<T: Copy>(data: &mut [u8], off: usize, count: usize) -> &mut [T] {
    let need = match count.checked_mul(core::mem::size_of::<T>()) {
        Some(cn) if count > 0 => match off.checked_add(cn) {
            Some(n) => n,
            None => return &mut [],
        },
        _ => return &mut [],
    };
    if data.len() < need {
        return &mut [];
    }
    let p = data.as_mut_ptr().wrapping_add(off);
    if (p as usize) % core::mem::align_of::<T>() != 0 {
        return &mut [];
    }
    // SAFETY: INV-EXT-EDGE-1 — bounds and alignment checked; the &mut
    // window rides the caller's exclusive slice.
    unsafe { core::slice::from_raw_parts_mut(p as *mut T, count) }
}

/// Typed `&T` view of a u32 array's start (the i_block header family;
/// INV-EXT-EDGE-2 — the array's own alignment covers any repr(C)
/// header with alignment ≤ 4).
pub fn u32_array_ref<T: Copy, const N: usize>(a: &[u32; N]) -> &T {
    assert!(
        core::mem::size_of::<T>() <= core::mem::size_of::<u32>() * N
            && core::mem::align_of::<T>() <= core::mem::align_of::<u32>(),
        "u32-array header view: type does not fit the array"
    );
    // SAFETY: INV-EXT-EDGE-2 — size and alignment hold by the assert.
    unsafe { &*(core::ptr::from_ref(a) as *const T) }
}

/// Exclusive twin of [`u32_array_ref`].
pub fn u32_array_ref_mut<T: Copy, const N: usize>(a: &mut [u32; N]) -> &mut T {
    assert!(
        core::mem::size_of::<T>() <= core::mem::size_of::<u32>() * N
            && core::mem::align_of::<T>() <= core::mem::align_of::<u32>(),
        "u32-array header view: type does not fit the array"
    );
    // SAFETY: INV-EXT-EDGE-2.
    unsafe { &mut *(core::ptr::from_mut(a) as *mut T) }
}

/// The all-zero bit pattern of an on-disk struct.
///
/// INV-EXT-EDGE-3: the CALLER'S type must be a `#[repr(C)]` struct of
/// integer/array-of-integer fields ONLY (no references, no enums with
/// niches, no floats needing NaN care) — the three on-disk Default
/// impls (Ext4InodeOnDisk, Ext4SuperBlockOnDisk, Ext4GroupDesc) hold
/// this by construction and name this edge at their audit site.
pub fn zeroed_on_disk<T: Copy>() -> T {
    // SAFETY: INV-EXT-EDGE-3 — the caller's type admits the zero
    // pattern (audited per call site).
    unsafe { core::mem::zeroed() }
}
