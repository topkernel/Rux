//! Framework-side IPC audited-edge zone (crate-flip rung: ipc).
//! The raw-page refcount touches the sysv shared-memory policy needs
//! against the vmemmap descriptors stay HERE (the net_edges pattern):
//! the service crate's shm policy calls these as SAFE functions; each
//! holds exactly one audited unsafe with its contract.
//!
//! INV-IPC-PAGEOP-1 (provenance): `phys` is a frame number that came
//! from the segment's own page set (get_zeroed_page allocations or
//! demand-filled PTE targets observed under the segment lock), so
//! `pfn_to_page_mut` yields the descriptor of an in-RAM page or null
//! (the null arm is the torn-teardown window and is skipped, matching
//! the historical behavior).

use crate::mm::{phys_to_pfn, pfn_to_page_mut};

/// Take one reference on the segment page backing `phys` (the
/// per-attachment pin installed by the shmat map loop, review 2R.6:
/// shmdt/exit's put_page must not free a page the segment still owns).
pub fn shm_page_get(phys: usize) {
    let page = pfn_to_page_mut(phys_to_pfn(phys));
    if !page.is_null() {
        // SAFETY: INV-IPC-PAGEOP-1 — pfn_to_page_mut returned the
        // vmemmap descriptor of an in-RAM frame (non-null arm).
        unsafe { (*page).get_page() };
    }
}

/// Drop the owner-side reference on the segment page backing `phys`.
/// Returns true when the decrement hit zero — the caller then returns
/// the frame to the buddy allocator.
pub fn shm_page_put(phys: usize) -> bool {
    let page = pfn_to_page_mut(phys_to_pfn(phys));
    if page.is_null() {
        // No descriptor: treat as "we own the free decision" (the
        // historical Drop arm).
        return true;
    }
    // SAFETY: INV-IPC-PAGEOP-1 — non-null vmemmap descriptor.
    unsafe { (*page).put_page() == 0 }
}
