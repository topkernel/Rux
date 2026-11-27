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
pub(crate) fn file_inode(file: &File) -> Option<&crate::fs::inode::Inode> {
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

// Helper only used by file_inode (Arc<Inode> -> &Inode).
trait AsDerefRefExt<T> {
    fn as_deref_ref(&self) -> Option<&T>;
}
impl<T> AsDerefRefExt<T> for Option<alloc::sync::Arc<T>> {
    fn as_deref_ref(&self) -> Option<&T> {
        self.as_ref().map(|a| &**a)
    }
}


// ===========================================================================
// Send/Sync commitments (moved verbatim from their home files by the
// 2026-11-27 mainline consolidation; each carries its original SAFETY
// note — the per-field lock/cell discipline arguments live with the
// type definitions, and these impls add no field access of their own).
// ===========================================================================

// SAFETY: BufferHead's mutable state (b_data) is protected by BufferState spinlock;
// other fields are atomic or only accessed under lock.
unsafe impl Send for crate::fs::bio::BufferHead {}

// SAFETY: all shared mutable state is protected by the BufferState spinlock;
// b_data is only mutated under lock or from a single thread.
unsafe impl Sync for crate::fs::bio::BufferHead {}

// SAFETY: BlockCache is only accessed via &self methods that use internal locking
// (per-bucket Spinlock and LRU Spinlock) and atomic count; raw pointers are
// confined to cache internals and never exposed without locks.
unsafe impl Send for crate::fs::bio::BlockCache {}

// SAFETY: all shared mutable state (hash buckets, LRU list) is protected by
// Spinlocks; raw pointer access is always under the appropriate bucket lock.
unsafe impl Sync for crate::fs::bio::BlockCache {}

// SAFETY: every mutable field sits behind a Spinlock; the closures are
// required to be Send + Sync at construction.
unsafe impl Send for crate::fs::debugfs::DebugFsNode {}

unsafe impl Sync for crate::fs::debugfs::DebugFsNode {}

unsafe impl Send for crate::fs::dentry::VfsMountInternal {}

unsafe impl Sync for crate::fs::dentry::VfsMountInternal {}

// SAFETY: the table's slots are shared-memory seqlock cells exactly like
// the inline `memo` array inside Sync Dentry; cross-thread access goes
// through the seqlock/children-lock protocol.
unsafe impl Send for crate::fs::dentry::BigMemoTable {}

unsafe impl Sync for crate::fs::dentry::BigMemoTable {}

// (INV-DENTRY-SEND-1: Dentry/VfsMountInternal/BigMemoTable Sync/Send
// are carried per-field — children/memo under their locks and seqlock,
// the retirement banks under the drain lock; no field races through
// a shared reference.)
unsafe impl Send for crate::fs::dentry::Dentry {}

unsafe impl Sync for crate::fs::dentry::Dentry {}

unsafe impl Send for crate::fs::dentry::DentryCache {}

unsafe impl Sync for crate::fs::dentry::DentryCache {}

// Send proofs for the raw-pointer registry types (moved from
// syscall/edges.rs with the types; same contracts: access only under
// the shard/registry locks).
unsafe impl Send for crate::fs::event_notify::EpollWakeShard {}

unsafe impl Send for crate::fs::event_notify::SignalfdWakeRegistry {}

// SAFETY: Ext4FileSystem is only accessed via &self methods that use bio layer
// locking; device is a raw pointer but only used for I/O through the bio cache.
unsafe impl Send for crate::fs::ext4::Ext4FileSystem {}

// SAFETY: all mutable access goes through bio::bread/brelse which uses internal
// locking; device pointer is read-only after initialization.
unsafe impl Sync for crate::fs::ext4::Ext4FileSystem {}

// SAFETY: File is only shared across threads when referenced through Arc,
// and all mutable access goes through Spinlock or UnsafeCell fields that
// are synchronized with IRQ-safe locking.
// (INV-FILE-SEND-1: File's Sync/Send are carried per-field — cells are
// write-once (INV-FILE-CELL-1), pos/cloexec/dirent_cache behind their
// locks, flags/f_owner/f_signum atomics, io_lock a real mutex. No
// field admits a data race through &File.)
unsafe impl Sync for crate::fs::file::File {}

unsafe impl Send for crate::fs::file::File {}

unsafe impl Send for crate::fs::fs_struct::FsStruct {}

unsafe impl Sync for crate::fs::fs_struct::FsStruct {}

unsafe impl Send for crate::fs::inode::Inode {}

unsafe impl Sync for crate::fs::inode::Inode {}

unsafe impl Send for crate::fs::inode::InodeCache {}

unsafe impl Sync for crate::fs::inode::InodeCache {}

unsafe impl Send for crate::fs::mount::VfsMount {}

unsafe impl Sync for crate::fs::mount::VfsMount {}

// SAFETY: ProcFSNode's mutable fields (children, data) are protected by Spinlocks;
// name is a &'static str and other fields are atomic or read-only after creation.
unsafe impl Send for crate::fs::procfs::ProcFSNode {}

// SAFETY: all shared mutable state is protected by internal Spinlocks;
// no data races are possible across threads/CPUs.
unsafe impl Sync for crate::fs::procfs::ProcFSNode {}

// SAFETY: RootFSPathCache uses internal Spinlock for mutable access;
// all fields are either locked or atomic.
unsafe impl Send for crate::fs::rootfs::RootFSPathCache {}

// SAFETY: all shared mutable state is protected by the internal Spinlock;
// no data races are possible across threads/CPUs.
unsafe impl Sync for crate::fs::rootfs::RootFSPathCache {}

// SAFETY: RootFSNode's mutable fields (data, children) are protected by Spinlocks;
// name uses UnsafeCell with callers ensuring exclusive access via parent-lock.
unsafe impl Send for crate::fs::rootfs::RootFSNode {}

// SAFETY: all shared mutable state is protected by internal Spinlocks or UnsafeCell;
// no data races are possible across threads/CPUs.
unsafe impl Sync for crate::fs::rootfs::RootFSNode {}

unsafe impl Send for crate::fs::superblock::SuperBlock {}

unsafe impl Sync for crate::fs::superblock::SuperBlock {}

unsafe impl Send for crate::fs::superblock::FsRegistry {}

unsafe impl Sync for crate::fs::superblock::FsRegistry {}

// SAFETY: all mutable state (children/attributes/parent) sits behind
// Spinlocks; closures are required to be Send + Sync at construction.
unsafe impl Send for crate::fs::sysfs::KObject {}

unsafe impl Sync for crate::fs::sysfs::KObject {}

// SAFETY: the raw superblock pointer is a leaked mount-lifetime
// allocation (Box::into_raw in create_mount_instance); Drop only
// touches an atomic through it.
unsafe impl Send for crate::fs::tmpfs::TmpfsPage {}

// SAFETY: mutable fields (pages/size/mode/children/atime/mtime) are protected
// by Spinlocks or atomics; name uses UnsafeCell mutated only under the
// parent's children lock.
unsafe impl Send for crate::fs::tmpfs::TmpfsNode {}

unsafe impl Sync for crate::fs::tmpfs::TmpfsNode {}

// SAFETY: the raw pointers are leaked superblocks (mount lifetime); they are
// only copied out and dereferenced under the registry lock.
unsafe impl Send for crate::fs::tmpfs::TmpfsInstances {}

// SAFETY: all mutable state is behind Spinlock/atomics; fn pointers are
// Send+Sync.
unsafe impl Sync for crate::fs::tty::TtyDevice {}

unsafe impl Send for crate::fs::tty::TtyDevice {}

