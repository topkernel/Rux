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
