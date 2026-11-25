//! The net stack's file→Arc identity helpers (framework side; crate-flip
//! rung 2): the raw adoption edge lives framework-side because the crate
//! is forbid-enforced. The type claim is the caller's ops-identity check.

use crate::fs::file::File;

/// Adopt the File's private-data Arc (INV-FS-FILEPRIV pattern: the
/// caller checked the ops-table identity first — the type proof).
pub fn file_arc<T>(file: &File) -> Option<alloc::sync::Arc<T>> {
    // SAFETY: the caller's ops identity check proves the type; the Arc
    // was leaked by into_raw at creation and the File holds a ref.
    unsafe {
        let p = (*file.private_data.get())?;
        if p.is_null() { return None; }
        let arc = alloc::sync::Arc::from_raw(p as *const T);
        // clone then forget to keep the File's reference alive
        let cloned = alloc::sync::Arc::clone(&arc);
        core::mem::forget(arc);
        Some(cloned)
    }
}

/// Take the Arc (consuming the File's reference — the close-path twin).
pub fn take_file_arc<T>(file: &File) -> Option<alloc::sync::Arc<T>> {
    // SAFETY: same identity contract; the take empties the cell (the
    // close-path's exactly-once reclaim).
    unsafe {
        let p = (*file.private_data.get()).take()?;
        Some(alloc::sync::Arc::from_raw(p as *const T))
    }
}
