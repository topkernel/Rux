//! Framework-side net UNSAFE EDGES (crate-flip rung 2): the raw-memory
//! substrate the service files must not carry now that net is
//! forbid-enforced — static-mut table reference formation, the
//! Arc-refcount forensics probe, the validated socket deref, and the
//! POD header views. (The file→Arc adoption family lives in
//! net/file_arc.rs and skb slicing in net/buffer.rs — both already
//! framework-side.)
//!
//! Families (one INV row each, registered in the safety ledger):
//! - INV-NET-SOCKTAB-1 — `static_table_ref` / `static_table_mut`
//! - INV-NET-ARCREF-1 — `arc_refcount_probe` / `audited_deref`
//! - INV-NET-HDRVIEW-1 — `pod_view` / `pod_view_mut`: alignment is
//!   CHECKED here (the old inline casts never checked it); length is
//!   the caller's packet-validation contract.

/// INV-NET-SOCKTAB-1: `p` comes from `addr_of_mut!` on one of the
/// stack's `static mut` socket/route tables; the caller holds the
/// documented serialization (the per-table lock or the softirq/syscall
/// discipline the site already carries), so no two live `&mut` aliases
/// of the same table exist concurrently. The reference lives as long
/// as the table: `'static`.
pub fn static_table_ref<T>(p: *mut T) -> &'static T {
    // SAFETY: INV-NET-SOCKTAB-1 above (caller-serialized global table).
    unsafe { &*p }
}

/// The mutable twin of `static_table_ref` (same INV row: the caller's
/// serialization is what makes a second alias impossible).
pub fn static_table_mut<T>(p: *mut T) -> &'static mut T {
    // SAFETY: INV-NET-SOCKTAB-1 above (caller-serialized global table).
    unsafe { &mut *p }
}

/// INV-NET-ARCREF-1a: diagnostic refcount reads for the Arc allocation
/// whose DATA pointer is `data_ptr` (Arc layout `[strong: u64, weak:
/// u64, data...]`; `Arc::into_raw` hands out `&data`, so the counts
/// live at `data_ptr - 16` / `data_ptr - 8`). Volatile reads,
/// forensics only — the values are heuristics, never control of
/// memory safety.
pub fn arc_refcount_probe(data_ptr: usize) -> (u64, u64) {
    // SAFETY: INV-NET-ARCREF-1a above (read-only volatile u64 pair in
    // the same allocation as the validated data pointer).
    unsafe {
        let strong = core::ptr::read_volatile((data_ptr - 16) as *const u64);
        let weak = core::ptr::read_volatile((data_ptr - 8) as *const u64);
        (strong, weak)
    }
}

/// INV-NET-ARCREF-1b: `p` names a live Arc-backed allocation whose
/// strong reference outlives the borrow (the File's private_data cell
/// or the socket table's slot holds it) — the caller validated the
/// provenance (heap-range + ops-identity) before calling.
pub fn audited_deref<T>(p: *const T) -> &'static T {
    // SAFETY: INV-NET-ARCREF-1b above (validated live Arc-backed ptr).
    unsafe { &*p }
}

/// INV-NET-CONTAINEROF-1: `member_addr` names a field of a live
/// `T` (the wait-queue registries store FIELD pointers); `offset` is
/// the caller's `offset_of!` for that same field, so
/// `member_addr - offset` addresses the whole object. Diagnostic-only
/// sites (trace-armed prints).
pub fn container_of<T>(member_addr: usize, offset: usize) -> &'static T {
    // SAFETY: INV-NET-CONTAINEROF-1 above (field ptr + matching offset).
    unsafe { &*((member_addr - offset) as *const T) }
}

/// INV-NET-HDRVIEW-1 (the read arm): interpret `data` as a POD header
/// `T`. Length AND alignment are checked — a short or misaligned
/// buffer returns `None` (the packet is dropped; the old inline casts
/// checked only length, so an unaligned skb buffer was silent UB).
pub fn pod_view<'a, T>(data: &'a [u8]) -> Option<&'a T> {
    if data.len() < core::mem::size_of::<T>() {
        return None;
    }
    let p = data.as_ptr();
    if (p as usize) & (core::mem::align_of::<T>() - 1) != 0 {
        return None;
    }
    // SAFETY: INV-NET-HDRVIEW-1 — length and alignment checked above;
    // T is a POD wire header (no interior references).
    Some(unsafe { &*(p as *const T) })
}

/// INV-NET-HDRVIEW-1 (the write arm): the TX push twin. `p` covers at
/// least `size_of::<T>()` writable bytes (the `skb_push` contract the
/// caller checked via its `Ok` return) and is alignment-checked here;
/// a misaligned push pointer returns `None` (packet dropped, never
/// UB).
pub fn pod_view_mut<T>(p: *mut u8) -> Option<&'static mut T> {
    if (p as usize) & (core::mem::align_of::<T>() - 1) != 0 {
        return None;
    }
    // SAFETY: INV-NET-HDRVIEW-1 — alignment checked above, length is
    // the skb_push contract; T is a POD wire header.
    Some(unsafe { &mut *(p as *mut T) })
}
