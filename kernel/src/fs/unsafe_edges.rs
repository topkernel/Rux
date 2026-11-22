//! Framework-owned raw edges for the filesystem service layer
//! (unsafe-slim L4, 2026-11-20; the FRAMEWORK/SERVICE doctrine:
//! service files — procfs, sysfs, tmpfs, devfs, ... — carry ZERO
//! `unsafe` keywords; every raw-pointer edge they used to inline lives
//! here behind one audited block per edge family).
//!
//! The edges consolidated here are exactly two families:
//! - inode-private data: `Inode::private_data` -> the owning
//!   filesystem's typed object (`privdata_ref`).
//! - file-private data: `File::private_data` -> the opening
//!   filesystem's typed object (`file_privdata_ref` /
//!   `file_privdata_take`).
//!
//! Everything else in the service layer is safe Rust over these views.
//! ext4's own block-layer edges stay in `fs::ext4` (the ext4 framework
//! files: BhRef, the i_block header views, FILERES) — this module owns
//! only the VFS-generic edges.

use crate::fs::file::File;
use crate::fs::inode::Inode;

/// INV-FS-PRIVDATA-1: `inode.private_data`, when `Some(p)`, is a raw
/// pointer to the filesystem-private object that the inode's own
/// filesystem installed at inode creation (iget/create sites, in-tree),
/// and it stays valid with unchanged shape until the inode's
/// `destroy_inode` hook reclaims it at the final `Arc<Inode>` drop.
/// The returned borrow is tied to the `&Inode` it came from — the
/// shared Arc the caller holds — so it cannot outlive the inode whose
/// Drop is the single reclaim site.
///
/// The TYPE claim is the caller's: each filesystem passes its own
/// private-data type, and the dispatch through its `INodeOps` table
/// guarantees the inode reached it belongs to that filesystem, whose
/// creation sites stored exactly that type.
pub(crate) fn privdata_ref<T>(inode: &Inode) -> Option<&T> {
    let p = inode.private_data?;
    // SAFETY: INV-FS-PRIVDATA-1 above (validity: creation-site install
    // to Drop reclaim; type: the ops-table dispatch fs match).
    Some(unsafe { &*(p as *const T) })
}

/// INV-FS-FILEPRIV-1: `File::private_data` is an `UnsafeCell` written
/// once at open time (before the File is published to an fd table) and
/// left untouched afterwards, except for the close path's single
/// take-and-reclaim (file release), which runs while the last Arc<File>
/// is exclusively owned — no shared borrow can be alive then. A shared
/// read through a shared `&File` is therefore race-free (same cell
/// discipline as `File::get_inode` / `File::get_ops`).
///
/// The TYPE claim is the caller's, as with `privdata_ref`: the FileOps
/// dispatch guarantees the file belongs to the filesystem whose open
/// hook stored the value.
pub(crate) fn file_privdata_ref<T>(file: &File) -> Option<&T> {
    // SAFETY: INV-FS-FILEPRIV-1 above (write-once cell, read shared).
    let p = unsafe { *file.private_data.get() }?;
    // SAFETY: type claim per INV-FS-FILEPRIV-1 (ops-table fs match).
    Some(unsafe { &*(p as *const T) })
}

/// The close-path reclaim twin of `file_privdata_ref` (INV-FS-FILEPRIV-1):
/// takes the stored pointer out of the cell and hands it back as the
/// owned `Box<T>` it was installed as, so the caller drops it. Must run
/// only from the file's release/close path (single-owner context).
pub(crate) fn file_privdata_take<T>(file: &File) -> Option<alloc::boxed::Box<T>> {
    // SAFETY: INV-FS-FILEPRIV-1: the take is the close-path reclaim;
    // callers run with exclusive logical ownership (release), so no
    // shared reader races the cell.
    let p = unsafe { (*file.private_data.get()).take() }?;
    // SAFETY: the value was installed by `Box::into_raw` in the same
    // filesystem's open hook; the type claim is the ops-table fs match
    // (INV-FS-FILEPRIV-1). Exactly-once: the take above emptied the
    // cell, so a second call returns None.
    Some(unsafe { alloc::boxed::Box::from_raw(p as *mut T) })
}

/// INV-FS-GLOBALSB-1: the per-filesystem GLOBAL_*_SB `AtomicPtr<T>`
/// globals hold a pointer installed exactly once at mount time
/// (`Box::into_raw` / leaked), never mutated afterwards except by the
/// umount/kill_sb teardown, which only runs after the mount tree is
/// detached (no service code can reach the FS then). A non-null load
/// therefore resolves to a valid `T` for `'static`. The TYPE claim is
/// the caller's (each global's accessor passes its own type).
pub(crate) fn static_ref<T>(p: *mut T) -> Option<&'static T> {
    if p.is_null() {
        return None;
    }
    // SAFETY: INV-FS-GLOBALSB-1 above (install-at-mount, immutable
    // until post-detach teardown).
    Some(unsafe { &*p })
}

/// The non-Option twin for call sites that null-checked the load
/// themselves (INV-FS-GLOBALSB-1): resolves the checked pointer. A null
/// pointer here would have been caught by the caller's check — passing
/// one anyway is a caller bug and dereferences null.
pub(crate) fn static_ref_unchecked<T>(p: *mut T) -> &'static T {
    // SAFETY: INV-FS-GLOBALSB-1; the caller checked non-null.
    unsafe { &*p }
}

/// The Arc twin of `file_privdata_ref` (INV-FS-FILEPRIV-1): a shared
/// view of the Arc-stashed object. The File's own reference keeps the
/// object alive for the returned borrow's lifetime.
pub(crate) fn file_privdata_arc_ref<T>(file: &File) -> Option<&T> {
    // SAFETY: INV-FS-FILEPRIV-1 (write-once cell) + the Arc the File
    // holds pins the object (the close-path take below is the only
    // drop site and runs with no shared borrow alive).
    let p = unsafe { *file.private_data.get() }?;
    if p.is_null() {
        return None;
    }
    // SAFETY: type claim per INV-FS-FILEPRIV-1 (ops-table fs match).
    Some(unsafe { &*(p as *const T) })
}

/// The Arc twin of `file_privdata_take` (INV-FS-FILEPRIV-1): reclaim
/// the Arc<T> the open hook stashed via `Arc::into_raw`, handing back
/// the owning Arc so the caller's drop decrements it. Close-path only.
pub(crate) fn file_privdata_arc_take<T>(file: &File) -> Option<alloc::sync::Arc<T>> {
    // SAFETY: INV-FS-FILEPRIV-1: the take is the close-path reclaim;
    // no shared reader can race the cell there. Exactly-once: the take
    // empties the slot.
    let p = unsafe { (*file.private_data.get()).take() }?;
    // SAFETY: the value was installed by Arc::into_raw in the same
    // filesystem's open hook (type claim: the ops-table fs match).
    Some(unsafe { alloc::sync::Arc::from_raw(p as *const T) })
}

/// The mutable twin of `file_privdata_ref` (INV-FS-FILEPRIV-1): an
/// exclusive view of the File-private object, for the release-time or
/// rewind-time mutations. The caller must hold the open file
/// description's serialization (io_lock / the FS's own single-reader
/// gate) — the pre-edge code performed the same mutation through the
/// raw pointer under the same conditions; this edge adds no new
/// guarantee and takes none away.
pub(crate) fn file_privdata_ref_mut<T>(file: &File) -> Option<&mut T> {
    // SAFETY: INV-FS-FILEPRIV-1 (write-once cell) — the value itself
    // is exclusively owned by this open file description; the caller
    // holds the description's serialization (see doc).
    let p = unsafe { *file.private_data.get() }?;
    if p.is_null() {
        return None;
    }
    // SAFETY: type claim per INV-FS-FILEPRIV-1; exclusivity per the
    // caller's serialization.
    Some(unsafe { &mut *(p as *mut T) })
}


/// INV-FS-FILEINODE-1: `File::inode` is an `UnsafeCell<Option<Arc<Inode>>>`
/// installed once at open/create time and immutable for the open file
/// description's lifetime (the Arc keeps the Inode alive; swaps happen
/// only through rename-with-replace, which installs a NEW cell value at
/// a different path's description). 30 call sites read it through
/// `(&*file.inode.get()).as_ref()` — this edge owns that one audit.
// M1a note: `pub` (was pub(crate)) — the mm policy crate (rux-mm) reads
// the inode through this edge in its fault routing; the audit row is
// unchanged (INV-FS-FILEINODE-1).
pub fn file_inode(file: &File) -> Option<&crate::fs::inode::Inode> {
    // SAFETY: INV-FS-FILEINODE-1 — write-once cell; the shared read is
    // sound for the description's whole lifetime (same shape as
    // file_privdata_ref).
    unsafe { (&*file.inode.get()).as_deref_ref() }
}

/// INV-FS-FILEINODE-2: the install twin of [`file_inode`] — writes the
/// cell ONCE, before the File is published to any fd table (the
/// constructor sites: open/create/procfs-memfile synth). No reader
/// exists at install time by construction.
pub(crate) fn file_inode_install(file: &File, inode: alloc::sync::Arc<crate::fs::inode::Inode>) {
    // SAFETY: INV-FS-FILEINODE-2 — pre-publication exclusive write.
    unsafe { *file.inode.get() = Some(inode) };
}

/// INV-FS-FILEPRIV-2: the take-and-reclaim twin of file_privdata_ref —
/// `Arc::into_raw`'s partner on the close path. Exactly-once: the cell
/// is emptied by the take, so a second call gets None; the from_raw
/// consumes the strong reference this File held (install was
/// Arc::into_raw — one strong count belongs to the cell).
pub(crate) fn file_privdata_take_arc<T>(file: &File) -> Option<alloc::sync::Arc<T>> {
    // SAFETY: INV-FS-FILEPRIV-1 (close-path exclusive ownership) +
    // INV-FS-FILEPRIV-2 (the take is the cell's single read of the
    // whole lifetime; from_raw's count ownership documented above).
    let p = unsafe { *file.private_data.get() }?;
    if p.is_null() {
        return None;
    }
    // SAFETY: type claim per INV-FS-FILEPRIV-1; the Arc count transfer
    // per INV-FS-FILEPRIV-2.
    Some(unsafe { alloc::sync::Arc::from_raw(p as *const T) })
}

/// The &'static twin of [`privdata_ref`] for LEAK-AT-INSTALL privdata
/// (INV-FS-PRIVDATA-1 + the leak discipline): devfs entries are wrapped
/// in Arc and deliberately leaked at install (never reclaimed), so the
/// object outlives every file — the 'static borrow is sound for them.
/// Every other filesystem's private_data MUST NOT use this edge.
pub(crate) fn privdata_ref_leaked<T>(inode: &Inode) -> Option<&'static T> {
    let p = inode.private_data?;
    // SAFETY: INV-FS-PRIVDATA-1 (validity + type) + the caller's
    // leak-at-install claim (the pointer's target lives forever).
    Some(unsafe { &*(p as *const T) })
}

/// The Arc-clone twin of [`file_inode`] (INV-FS-FILEINODE-1): takes a
/// strong reference for storage beyond the call.
pub(crate) fn file_inode_arc(file: &File) -> Option<alloc::sync::Arc<crate::fs::inode::Inode>> {
    // SAFETY: INV-FS-FILEINODE-1 (write-once cell); the clone is an
    // ordinary Arc read.
    unsafe { (*file.inode.get()).clone() }
}

/// INV-FS-RASTATE-1: `File::ra` is an `UnsafeCell<Option<ReadAheadState>>`
/// written to `None` once in `File::new` and mutated afterwards only by
/// the description's reader — every caller holds the file's `io_lock`
/// (the read paths' serialization), which excludes any other mutable or
/// shared borrow of the state for the returned reference's scope. The
/// cell is a File field, so the borrow cannot outlive the `&File`.
pub(crate) fn file_ra_state_mut(
    file: &File,
) -> &mut Option<crate::fs::readahead::ReadAheadState> {
    // SAFETY: INV-FS-RASTATE-1 — the io_lock the caller holds is the
    // exclusivity proof for this &mut.
    unsafe { &mut *file.ra.get() }
}

// Helper only used by file_inode (Arc<Inode> -> &Inode).
trait AsDerefRefExt<T> {
    fn as_deref_ref(&self) -> Option<&T>;
}
impl<T> AsDerefRefExt<T> for Option<alloc::sync::Arc<T>> {
    fn as_deref_ref(&self) -> Option<&T> {
        self.as_ref().map(|a| &**a)
    }
}
