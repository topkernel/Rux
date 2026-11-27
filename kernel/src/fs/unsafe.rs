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



// ===========================================================================
// Generic reclaim / construction primitives (2026-11-27 mainline toolkit).
// One INV each; call sites restate the caller obligation in their local
// SAFETY comment.
// ===========================================================================

/// INV-FS-RAWTAKE-1: reclaim twin of `Box::into_raw` / `Box::leak`.
/// Pointer provenance: `p` is the exact pointer one `Box::into_raw`
/// produced for a `Box<T>` (including the `*mut SuperBlock`
/// type-erasures the mount/kill_sb callbacks do). Caller obligations:
/// (a) this is the object's SINGLE reclaim (run at most once — a second
/// call double-frees); (b) no borrow of the object outlives the call
/// (the returned Box owns it exclusively from here); (c) the type T is
/// the exact install-time type.
pub(crate) fn box_take<T>(p: *mut T) -> alloc::boxed::Box<T> {
    // SAFETY: INV-FS-RAWTAKE-1 above.
    unsafe { alloc::boxed::Box::from_raw(p) }
}

/// INV-FS-RAWTAKE-2: reclaim twin of `Arc::into_raw`. The stashed
/// pointer owns one strong count (the install site transferred it);
/// from_raw reconstructs the owning Arc so the caller's drop releases
/// it. Exactly-once for the same reason as RAWTAKE-1; type claim is the
/// caller's (the ops-table dispatch match).
pub(crate) fn arc_take<T>(p: *const T) -> alloc::sync::Arc<T> {
    // SAFETY: INV-FS-RAWTAKE-2 above.
    unsafe { alloc::sync::Arc::from_raw(p) }
}

/// INV-FS-ZEROPOD-1: all-zero-bytes constructor for `#[repr(C)]` POD
/// types. Caller obligation: T is repr(C) and composed only of integer
/// / bool / float / array-of-those fields (no references, no NonNull,
/// no niche enums) — the zeroed byte pattern is a valid `T`.
pub(crate) fn zeroed_repr_c<T>() -> T {
    // SAFETY: INV-FS-ZEROPOD-1 (caller asserts the POD shape).
    unsafe { core::mem::zeroed() }
}


// ---------------------------------------------------------------------------
// 2026-11-27 consolidation wave 2: generic view/access primitives + the
// buffer-head data family + File cell twins.
// ---------------------------------------------------------------------------

/// INV-FS-RAWVIEW-3: generic raw-pointer view. Provenance: the pointer
/// comes from an install site documented at its storage (private_data
/// cells, leaked statics, per-CPU slots, published snapshots). Caller
/// obligations: non-null, valid for the returned borrow's whole
/// lifetime, no concurrent writer through the same memory for the
/// &'a T claimed, and the type claim (what the install site stored).
pub(crate) fn raw_ref<'a, T>(p: *const T) -> &'a T {
    // SAFETY: INV-FS-RAWVIEW-3 (caller proves the obligations above).
    unsafe { &*p }
}

/// The exclusive twin of [`raw_ref`] (INV-FS-RAWVIEW-3): caller must in
/// addition hold exclusive logical ownership of the object for the
/// borrow's lifetime.
pub(crate) fn raw_mut<'a, T>(p: *mut T) -> &'a mut T {
    // SAFETY: INV-FS-RAWVIEW-3 + the caller's exclusivity.
    unsafe { &mut *p }
}

/// INV-FS-VOLATILE-1: volatile access to an address the caller proves
/// is mapped and intended for volatile access (per-CPU areas, device
/// pages, stack walks). Volatile semantics preserved verbatim.
pub(crate) fn volatile_read<T: Copy>(p: *const T) -> T {
    // SAFETY: INV-FS-VOLATILE-1.
    unsafe { core::ptr::read_volatile(p) }
}

/// Indexed u8 twin of [`volatile_read`] (INV-FS-VOLATILE-1): the
/// pointer arithmetic happens inside the edge so no raw `add` leaks
/// into safe files. base+k must be mapped (stack-walk bounds are the
/// caller's check).
pub(crate) fn volatile_read_at<T: Copy>(base: *const T, k: usize) -> T {
    // SAFETY: INV-FS-VOLATILE-1 (indexed form; base+k mapped per the
    // caller's bounds check).
    unsafe { core::ptr::read_volatile(base.add(k)) }
}

/// The write twin of [`volatile_read`] (INV-FS-VOLATILE-1).
pub(crate) fn volatile_write<T: Copy>(p: *mut T, v: T) {
    // SAFETY: INV-FS-VOLATILE-1.
    unsafe { core::ptr::write_volatile(p, v) }
}

/// INV-FS-UACCESS-1: user-memory copy. `dst` comes from the syscall
/// layer after access_ok validation over at least `src.len()` bytes;
/// faults are taken safely through the exception table.
pub(crate) fn copy_to_user_bytes(dst: *mut u8, src: &[u8]) -> usize {
    // SAFETY: INV-FS-UACCESS-1.
    unsafe { crate::arch::uaccess::copy_to_user(dst, src.as_ptr(), src.len()) }
}

/// The user-to-kernel twin of [`copy_to_user_bytes`] (INV-FS-UACCESS-1):
/// `dst` is a caller-provided kernel buffer of src-given length; faults
/// resolve through the exception table.
pub(crate) fn copy_from_user_bytes(dst: &mut [u8], src: *const u8) -> usize {
    // SAFETY: INV-FS-UACCESS-1 (kernel dst, user src).
    unsafe { crate::arch::uaccess::copy_from_user(dst.as_mut_ptr(), src, dst.len()) }
}

/// INV-FS-CELL-1: borrow through an `UnsafeCell` whose discipline is
/// write-once-then-read or externally serialized (name cells, dentry
/// cells — see the type's own docs). Caller holds that serialization.
pub(crate) fn cell_ref<'a, T: ?Sized>(c: &core::cell::UnsafeCell<T>) -> &'a T {
    // SAFETY: INV-FS-CELL-1.
    unsafe { &*c.get() }
}

/// The assign twin of [`cell_ref`] (INV-FS-CELL-1): caller holds the
/// exclusive side of the cell's discipline for the assignment.
pub(crate) fn cell_assign<T>(c: &core::cell::UnsafeCell<T>, v: T) {
    // SAFETY: INV-FS-CELL-1 (exclusivity per the caller).
    unsafe { *c.get() = v }
}

/// INV-FS-BHDATA-1: the buffer-head data family. The bh is CHECKED OUT
/// by the caller (`bio::bread` ... `bio::brelse` pair); every borrow
/// returned here must end before the brelse. The b_data Vec is sized
/// to the block at bh creation, so `bh_view::<T>` obligations are: T
/// fits in one block (size_of::<T>() <= b_data.len()), repr(C) POD,
/// and the block really holds that layout (the read path's contract).
pub(crate) fn bh_data<'a>(bh: *mut crate::fs::bio::BufferHead) -> &'a alloc::vec::Vec<u8> {
    // SAFETY: INV-FS-BHDATA-1 (checked-out bh, borrow ends by brelse).
    unsafe { &(*bh).b_data }
}

/// The typed-view twin of [`bh_data`] (INV-FS-BHDATA-1).
pub(crate) fn bh_view<'a, T>(bh: *mut crate::fs::bio::BufferHead) -> &'a T {
    // SAFETY: INV-FS-BHDATA-1 (T fits the block, caller proves layout).
    unsafe { &*((*bh).b_data.as_ptr() as *const T) }
}

/// The byte-slice twin of [`bh_data`] (INV-FS-BHDATA-1): len must not
/// exceed the block size.
pub(crate) fn bh_bytes<'a>(bh: *mut crate::fs::bio::BufferHead, len: usize) -> &'a [u8] {
    // SAFETY: INV-FS-BHDATA-1 (len <= block size per caller).
    unsafe { core::slice::from_raw_parts((*bh).b_data.as_ptr(), len) }
}

/// INV-FS-FILEPRIV-3: clone a strong reference out of the file-private
/// Arc cell WITHOUT taking it — the close path's take stays the single
/// reclaim. from_raw + clone + forget is the documented inc-ref recipe
/// (the clone raises the count; forget neutralizes the reconstructed
/// phantom ownership). Same write-once cell as INV-FS-FILEPRIV-1.
pub(crate) fn file_privdata_arc_clone<T>(file: &File) -> Option<alloc::sync::Arc<T>> {
    // SAFETY: INV-FS-FILEPRIV-1 (write-once cell, read shared).
    let p = unsafe { *file.private_data.get() }?;
    if p.is_null() {
        return None;
    }
    // SAFETY: INV-FS-FILEPRIV-3 above (clone+forget inc-ref recipe).
    let arc = unsafe { alloc::sync::Arc::from_raw(p as *const T) };
    let cloned = arc.clone();
    core::mem::forget(arc);
    Some(cloned)
}

/// INV-FS-FILEPRIV-4: take the RAW private-data pointer out of the
/// cell (emptying it, exactly-once) for the caller to reclaim with the
/// matching typed edge (arc_take / box_take). Close-path only.
pub(crate) fn file_privdata_ptr_take(file: &File) -> Option<*mut u8> {
    // SAFETY: INV-FS-FILEPRIV-1 (close-path exclusive take).
    unsafe { (*file.private_data.get()).take() }
}

/// INV-FS-KCOPY-1: kernel-side page copies. `kva` is a live, mapped
/// kernel page (sharedmem/shared-fault backing); the slice side is
/// caller-valid over the same length. Copies stay within one page per
/// call (the callers' in-page chunking).
pub(crate) fn page_copy_to_slice(kva: *const u8, dst: &mut [u8]) {
    // SAFETY: INV-FS-KCOPY-1.
    unsafe { core::ptr::copy_nonoverlapping(kva, dst.as_mut_ptr(), dst.len()) }
}

/// The write twin of [`page_copy_to_slice`] (INV-FS-KCOPY-1).
pub(crate) fn page_copy_from_slice(kva: *mut u8, src: &[u8]) {
    // SAFETY: INV-FS-KCOPY-1.
    unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), kva, src.len()) }
}

/// INV-FS-ZERO-1: byte-fill of a mapped kernel span (device/shared
/// pages, segment BSS). `dst` mapped+writable over `n` bytes.
pub(crate) fn fill_bytes(dst: *mut u8, byte: u8, n: usize) {
    // SAFETY: INV-FS-ZERO-1.
    unsafe { core::ptr::write_bytes(dst, byte, n) }
}

/// The offset twin of [`fill_bytes`] (INV-FS-ZERO-1): the `add`
/// happens inside the edge; base+off..+n must be mapped+writable.
pub(crate) fn fill_bytes_at(base: *mut u8, off: usize, byte: u8, n: usize) {
    // SAFETY: INV-FS-ZERO-1 (indexed form).
    unsafe { core::ptr::write_bytes(base.add(off), byte, n) }
}

/// INV-FS-SEGMENT-1: ELF segment fill — copy `filesz` bytes of file
/// image then zero the BSS tail up to `memsz`, both into the freshly
/// mapped exec page range `vaddr` (the exec path mapped and
/// bounds-checked the span before loading; see elf.rs load_segment).
pub(crate) fn load_segment_fill(
    vaddr: u64,
    data: &[u8],
    offset: usize,
    filesz: usize,
    memsz: usize,
) -> Result<(), crate::fs::elf::ElfError> {
    // SAFETY: INV-FS-SEGMENT-1 (exec-path-mapped destination span).
    unsafe {
        if filesz > 0 {
            let src = data.as_ptr().add(offset);
            let dst = vaddr as *mut u8;
            core::ptr::copy_nonoverlapping(src, dst, filesz);
        }
        if memsz > filesz {
            let bss_start = vaddr.checked_add(filesz as u64).ok_or(crate::fs::elf::ElfError::InvalidSegment)?;
            let bss_size = memsz - filesz;
            core::ptr::write_bytes(bss_start as *mut u8, 0, bss_size);
        }
    }
    Ok(())
}

/// INV-FS-LISTHEAD-1: intrusive ListHead splice/remove primitives.
/// Caller obligations: the node is a live, properly linked ListHead of
/// some list (splice: `new` is NOT currently linked; remove: `entry`
/// IS linked); the list is protected against concurrent mutation by
/// the owner's lock (jbd2 journal state lock).
pub(crate) fn lh_insert_after(
    self_: *mut crate::fs::jbd2::journal::ListHead,
    new_: *mut crate::fs::jbd2::journal::ListHead,
) {
    // SAFETY: INV-FS-LISTHEAD-1.
    unsafe {
        (*new_).next = (*self_).next;
        (*new_).prev = self_;
        if !(*self_).next.is_null() {
            (*(*self_).next).prev = new_;
        }
        (*self_).next = new_;
    }
}

/// The before-twin of [`lh_insert_after`] (INV-FS-LISTHEAD-1).
pub(crate) fn lh_insert_before(
    self_: *mut crate::fs::jbd2::journal::ListHead,
    new_: *mut crate::fs::jbd2::journal::ListHead,
) {
    // SAFETY: INV-FS-LISTHEAD-1.
    unsafe {
        (*new_).next = self_;
        (*new_).prev = (*self_).prev;
        if !(*self_).prev.is_null() {
            (*(*self_).prev).next = new_;
        }
        (*self_).prev = new_;
    }
}

/// The unlink primitive of the [`lh_insert_after`] family
/// (INV-FS-LISTHEAD-1): no-op on null-sentinel ends.
pub(crate) fn lh_remove(entry: *mut crate::fs::jbd2::journal::ListHead) {
    // SAFETY: INV-FS-LISTHEAD-1.
    unsafe {
        if !(*entry).next.is_null() && !(*entry).prev.is_null() {
            (*(*entry).next).prev = (*entry).prev;
            (*(*entry).prev).next = (*entry).next;
        }
    }
}


// ---------------------------------------------------------------------------
// 2026-11-27 consolidation wave 3: cells, chunk views, list shims, slices.
// ---------------------------------------------------------------------------

/// INV-FS-FILEPRIV-5: raw cell read/assign/clear twins for the
/// File::private_data UnsafeCell (write-once + close-path-take
/// discipline, INV-FS-FILEPRIV-1). The caller converts the taken raw
/// pointer with the matching typed reclaim edge.
pub(crate) fn file_privdata_cell_read(file: &File) -> Option<*mut u8> {
    // SAFETY: INV-FS-FILEPRIV-5.
    unsafe { *file.private_data.get() }
}
pub(crate) fn file_privdata_cell_assign(file: &File, p: Option<*mut u8>) {
    // SAFETY: INV-FS-FILEPRIV-5 (install before publication / close path).
    unsafe { *file.private_data.get() = p }
}
pub(crate) fn file_privdata_cell_clear(file: &File) {
    // SAFETY: INV-FS-FILEPRIV-5 (close-path clear).
    unsafe { *file.private_data.get() = None }
}

/// INV-FS-FILEINODE-3: the File::dentry cell — installed once at
/// open_tree/attach time and immutable for the description's lifetime
/// (same discipline as INV-FS-FILEINODE-1). Clones the Arc out.
pub(crate) fn file_dentry_arc(file: &File) -> Option<alloc::sync::Arc<crate::fs::dentry::Dentry>> {
    // SAFETY: INV-FS-FILEINODE-3 (write-once cell).
    unsafe { (*file.dentry.get()).clone() }
}

/// INV-FS-RAWTAKE-3: clone a strong ref from a stashed Arc pointer and
/// RE-STASH the original (from_raw + clone + into_raw: the stashed
/// count is untouched, the caller gets a fresh owning Arc). Provenance:
/// the pointer owns exactly one strong count for the object's life.
pub(crate) fn arc_clone_leak<T>(p: *const T) -> alloc::sync::Arc<T> {
    // SAFETY: INV-FS-RAWTAKE-3 (the count round-trip documented above).
    unsafe {
        let arc = alloc::sync::Arc::from_raw(p);
        let cloned = arc.clone();
        let _ = alloc::sync::Arc::into_raw(arc);
        cloned
    }
}

/// INV-FS-BUFCHUNK-1: borrow `len` bytes at offset `off` inside a
/// FileBuffer chunk (heap Vec or pooled page; capacity checked at
/// chunk creation). The inode data lock serializes chunk access, and
/// the returned borrow dies before the lock releases (same scope).
pub(crate) fn chunk_view<'a>(fc: *mut crate::fs::buffer::FileChunk, off: usize, len: usize) -> &'a [u8] {
    // SAFETY: INV-FS-BUFCHUNK-1.
    unsafe { core::slice::from_raw_parts((*fc).base_ptr().add(off), len) }
}

/// The mut twin of [`chunk_view`] (INV-FS-BUFCHUNK-1; caller holds the
/// data lock exclusively).
pub(crate) fn chunk_view_mut<'a>(fc: *mut crate::fs::buffer::FileChunk, off: usize, len: usize) -> &'a mut [u8] {
    // SAFETY: INV-FS-BUFCHUNK-1.
    unsafe { core::slice::from_raw_parts_mut((*fc).base_ptr().add(off), len) }
}

/// INV-FS-VECFILL-1: fresh zeroed `Vec<u8>`: with_capacity reserves
/// len bytes (no realloc for the writes below), write_bytes fills the
/// reserved span with zeros (valid for u8), set_len commits.
pub(crate) fn zeroed_vec(len: usize) -> alloc::vec::Vec<u8> {
    let mut v = alloc::vec::Vec::with_capacity(len);
    // SAFETY: INV-FS-VECFILL-1 (reserved capacity, zeroed bytes).
    unsafe {
        core::ptr::write_bytes(v.as_mut_ptr(), 0, len);
        v.set_len(len);
    }
    v
}

/// INV-FS-VECLEN-1: commit `len` bytes on a Vec that JUST reserved
/// capacity >= len (the caller's with_capacity is the proof); contents
/// stay uninitialized as the caller wrote them.
pub(crate) fn vec_set_len(v: &mut alloc::vec::Vec<u8>, len: usize) {
    // SAFETY: INV-FS-VECLEN-1.
    unsafe { v.set_len(len) }
}

/// INV-FS-BHDATA-2: the bh blocknr field read (checked-out bh per
/// INV-FS-BHDATA-1).
pub(crate) fn bh_blocknr(bh: *mut crate::fs::bio::BufferHead) -> u64 {
    // SAFETY: INV-FS-BHDATA-1 (checked-out bh).
    unsafe { (*bh).b_blocknr }
}

/// INV-FS-BHDATA-3: the exclusive b_data twin (checked-out bh).
pub(crate) fn bh_data_mut<'a>(bh: *mut crate::fs::bio::BufferHead) -> &'a mut alloc::vec::Vec<u8> {
    // SAFETY: INV-FS-BHDATA-1.
    unsafe { &mut (*bh).b_data }
}

/// INV-FS-SLICECAST-1: reinterpret a byte slice as [T]. Caller
/// obligations: T is repr(C) POD, size_of::<T>() divides data.len(),
/// and the backing storage is T-aligned (bh blocks and fresh Vec<u8>
/// reservations are page/malloc aligned).
pub(crate) fn slice_as_t<'a, T>(data: &[u8]) -> &'a [T] {
    // SAFETY: INV-FS-SLICECAST-1.
    unsafe {
        core::slice::from_raw_parts(data.as_ptr() as *const T, data.len() / core::mem::size_of::<T>())
    }
}

/// The mut twin of [`slice_as_t`] (INV-FS-SLICECAST-1).
pub(crate) fn slice_as_t_mut<'a, T>(data: &mut [u8]) -> &'a mut [T] {
    // SAFETY: INV-FS-SLICECAST-1.
    unsafe {
        core::slice::from_raw_parts_mut(data.as_ptr() as *mut T, data.len() / core::mem::size_of::<T>())
    }
}

/// The offset twins of the page-copy family (INV-FS-KCOPY-1): the
/// in-page pointer arithmetic happens inside the edge; off+n stays
/// within the one page (the callers' chunking proves it).
pub(crate) fn page_copy_to_slice_at(kva: *const u8, off: usize, dst: &mut [u8]) {
    // SAFETY: INV-FS-KCOPY-1 (indexed form).
    unsafe { core::ptr::copy_nonoverlapping(kva.add(off), dst.as_mut_ptr(), dst.len()) }
}
pub(crate) fn page_copy_from_slice_at(kva: *mut u8, off: usize, src: &[u8]) {
    // SAFETY: INV-FS-KCOPY-1 (indexed form).
    unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), kva.add(off), src.len()) }
}

/// INV-FS-SYSC-LIST-1: crate::list::ListHead raw shims for the
/// event-notify chain walk (INV-SYSC-1: nodes are live under the
/// registry lock; capture-before-body keeps neighbors alive).
pub(crate) fn lh_next(p: *const crate::list::ListHead) -> *mut crate::list::ListHead {
    // SAFETY: INV-FS-SYSC-LIST-1.
    unsafe { (*p).next }
}
pub(crate) fn lh_add_tail(node: *mut crate::list::ListHead, head: *mut crate::list::ListHead) {
    // SAFETY: INV-FS-SYSC-LIST-1.
    unsafe { (*node).add_tail(head) }
}
pub(crate) fn lh_del(node: *mut crate::list::ListHead) {
    // SAFETY: INV-FS-SYSC-LIST-1.
    unsafe { (*node).del() }
}


/// INV-FS-BHSTATE-1: set/clear a state bit on a checked-out bh
/// (INV-FS-BHDATA-1 discipline; the bit ops are the BH_Req in-flight
/// marking the brelse path relies on).
pub(crate) fn bh_set_state_bit(bh: *mut crate::fs::bio::BufferHead, bit: u8) {
    // SAFETY: INV-FS-BHSTATE-1.
    unsafe { (*bh).set_state_bit(bit) }
}

/// INV-FS-JHANDLE-1: the per-task current jbd2 handle slot (a task-
/// local cell: the handle is set at transaction start on the same
/// task and cleared at stop_handle on it — same-task access only).
pub(crate) fn current_jbd2_handle() -> Option<*mut crate::fs::jbd2::Handle> {
    // SAFETY: INV-FS-JHANDLE-1 (task-local cell read).
    unsafe { crate::fs::ext4::namei::get_current_handle() }
}


/// INV-EXT4-JHANDLE-1 (consolidated): the per-task journal-handle cell
/// family. sched::current() returns the RUNNING task's pointer; the
/// handle pointer is stored by the SAME task that created the
/// transaction, lives on that task's stack for the enclosing ext4 op,
/// and the set/clear/get rows all run task-local — no cross-task
/// aliasing exists by construction.
pub(crate) fn jbd2_handle_set(handle: *mut crate::fs::jbd2::Handle) {
    if let Some(task) = crate::sched::current() {
        // SAFETY: INV-EXT4-JHANDLE-1 (same-task cell write).
        crate::process::unsafe_edges::borrow_task_mut(task).journal_handle.set(handle);
    }
}

/// The clear twin of [`jbd2_handle_set`] (INV-EXT4-JHANDLE-1).
pub(crate) fn jbd2_handle_clear() {
    if let Some(task) = crate::sched::current() {
        // SAFETY: INV-EXT4-JHANDLE-1.
        crate::process::unsafe_edges::borrow_task_mut(task).journal_handle.set(core::ptr::null_mut());
    }
}

/// The get twin of [`jbd2_handle_set`] (INV-EXT4-JHANDLE-1).
pub(crate) fn jbd2_handle_get() -> Option<*mut crate::fs::jbd2::Handle> {
    let task = crate::sched::current()?;
    // SAFETY: INV-EXT4-JHANDLE-1.
    let ptr = crate::process::unsafe_edges::borrow_task(task).journal_handle.get();
    if ptr.is_null() { None } else { Some(ptr) }
}


/// INV-FS-JDIRTY-1: journal dirty-metadata registration. `handle` is
/// the current task's live transaction handle (INV-EXT4-JHANDLE-1 —
/// the caller got it from jbd2_handle_get in the same task), `bh` is a
/// checked-out metadata buffer (INV-FS-BHDATA-1). Pure registration
/// bookkeeping; no extra validity claims beyond those two.
pub(crate) fn jbd2_dirty_metadata(
    handle: *mut crate::fs::jbd2::Handle,
    bh: *mut crate::fs::bio::BufferHead,
) -> Result<(), i32> {
    // SAFETY: INV-FS-JDIRTY-1 (deref forms the &mut the API takes).
    unsafe { crate::fs::jbd2::jbd2_journal_dirty_metadata(&mut *handle, bh) }
}
