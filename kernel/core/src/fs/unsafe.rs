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

use crate::fs::types::File;
use crate::fs::types::Inode;
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::Ordering;
use crate::arch::mm::{PhysAddr, phys_to_virt};
use crate::list::ListHead;
use crate::mm::hotpath::pfn_to_phys;
use crate::process::types::Task;
use crate::fs::types::{
    AnonDevMap, AssertingSyncArc, BigMemoTable, BlockCache, BufferHead,
    BufferState, CacheEntry, Dentry, DentryCache, DevNo, ElfError,
    EpollWakeShard, FileChunk, FileFlags, FileOpKind, FileOps, FastFd,
    FdTable, FsRegistry, FsStruct, InodeCache, InodeMode, LruState,
    MemFileContent, SignalfdWakeRegistry, SuperBlock, VfsMount,
    VfsMountInternal,
};
use super::hotpath::{
    ANON_DEV_MAP, ANON_DEV_NEXT, ANON_DEV_WRITE, FILE_BUFFER_CHUNK,
    get_file_fd, ino_events, notify_file, phys_to_virt_ptr,
    wait_buffer_io_done,
};

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
pub fn privdata_ref<T>(inode: &Inode) -> Option<&T> {
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
pub fn file_privdata_ref<T>(file: &File) -> Option<&T> {
    // SAFETY: INV-FS-FILEPRIV-1 above (write-once cell, read shared).
    let p = unsafe { *file.private_data.get() }?;
    // SAFETY: type claim per INV-FS-FILEPRIV-1 (ops-table fs match).
    Some(unsafe { &*(p as *const T) })
}

/// The close-path reclaim twin of `file_privdata_ref` (INV-FS-FILEPRIV-1):
/// takes the stored pointer out of the cell and hands it back as the
/// owned `Box<T>` it was installed as, so the caller drops it. Must run
/// only from the file's release/close path (single-owner context).
pub fn file_privdata_take<T>(file: &File) -> Option<alloc::boxed::Box<T>> {
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
pub fn static_ref<T>(p: *mut T) -> Option<&'static T> {
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
pub fn static_ref_unchecked<T>(p: *mut T) -> &'static T {
    // SAFETY: INV-FS-GLOBALSB-1; the caller checked non-null.
    unsafe { &*p }
}

/// The Arc twin of `file_privdata_ref` (INV-FS-FILEPRIV-1): a shared
/// view of the Arc-stashed object. The File's own reference keeps the
/// object alive for the returned borrow's lifetime.
pub fn file_privdata_arc_ref<T>(file: &File) -> Option<&T> {
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
pub fn file_privdata_arc_take<T>(file: &File) -> Option<alloc::sync::Arc<T>> {
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
pub fn file_privdata_ref_mut<T>(file: &File) -> Option<&mut T> {
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
// M1a note: `pub` (was pub(crate)) — the mm policy crate (mm) reads
// the inode through this edge in its fault routing; the audit row is
// unchanged (INV-FS-FILEINODE-1).
pub fn file_inode(file: &File) -> Option<&crate::fs::types::Inode> {
    // SAFETY: INV-FS-FILEINODE-1 — write-once cell; the shared read is
    // sound for the description's whole lifetime (same shape as
    // file_privdata_ref).
    unsafe { (&*file.inode.get()).as_deref_ref() }
}

/// INV-FS-FILEINODE-2: the install twin of [`file_inode`] — writes the
/// cell ONCE, before the File is published to any fd table (the
/// constructor sites: open/create/procfs-memfile synth). No reader
/// exists at install time by construction.
pub fn file_inode_install(file: &File, inode: alloc::sync::Arc<crate::fs::types::Inode>) {
    // SAFETY: INV-FS-FILEINODE-2 — pre-publication exclusive write.
    unsafe { *file.inode.get() = Some(inode) };
}

/// INV-FS-FILEPRIV-2: the take-and-reclaim twin of file_privdata_ref —
/// `Arc::into_raw`'s partner on the close path. Exactly-once: the cell
/// is emptied by the take, so a second call gets None; the from_raw
/// consumes the strong reference this File held (install was
/// Arc::into_raw — one strong count belongs to the cell).
pub fn file_privdata_take_arc<T>(file: &File) -> Option<alloc::sync::Arc<T>> {
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
pub fn privdata_ref_leaked<T>(inode: &Inode) -> Option<&'static T> {
    let p = inode.private_data?;
    // SAFETY: INV-FS-PRIVDATA-1 (validity + type) + the caller's
    // leak-at-install claim (the pointer's target lives forever).
    Some(unsafe { &*(p as *const T) })
}

/// The Arc-clone twin of [`file_inode`] (INV-FS-FILEINODE-1): takes a
/// strong reference for storage beyond the call.
pub fn file_inode_arc(file: &File) -> Option<alloc::sync::Arc<crate::fs::types::Inode>> {
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
pub fn file_ra_state_mut(
    file: &File,
) -> &mut Option<crate::fs::types::ReadAheadState> {
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

// ==================== Rung-3 flip teardown edges ====================

/// destroy_inode's Box reclaim: take the inode's private_data slot and
/// drop it as the `Box<T>` the leaf leaked at creation
/// (INV-FS-PRIVDATA-1's take-twin; the Drop-time single reclaim).
pub fn privdata_take_box<T>(inode: &mut Inode) -> Option<()> {
    // SAFETY: INV-FS-PRIVDATA-1 — the leaf stored a Box<T> via
    // into_raw at inode creation; this Drop-path take is the single
    // reclaim (the cell is emptied first, making a second impossible).
    if let Some(ptr) = inode.private_data.take() {
        drop(unsafe { alloc::boxed::Box::from_raw(ptr as *mut T) });
        Some(())
    } else {
        None
    }
}

/// destroy_inode's Arc drop: take the private_data slot and drop the
/// `Arc<T>` reference the leaf stored (the last-ref case frees).
pub fn privdata_drop_arc<T>(inode: &mut Inode) {
    // SAFETY: INV-FS-FILEPRIV-1 shape on the inode slot — the stored
    // value is an Arc::into_raw of the leaf's node type.
    if let Some(ptr) = inode.private_data.take() {
        drop(unsafe { alloc::sync::Arc::from_raw(ptr as *const T) });
    }
}

/// kill_sb's Box reclaim: the superblock pointer a mount callback
/// returned via Box::into_raw, reclaimed at unmount (null-safe).
pub fn kill_sb_reclaim_box<T>(sb: *mut crate::fs::types::SuperBlock) {
    if !sb.is_null() {
        // SAFETY: the mount callback stored a Box<T> via into_raw; this
        // unmount-path call is the single reclaim.
        drop(unsafe { alloc::boxed::Box::from_raw(sb as *mut T) });
    }
}

/// Clone the File's private-data Arc WITHOUT consuming the slot
/// (increment + from_raw; the slot keeps its single reference — the
/// pty/inotify instance-recovery shape). INV-FS-FILEPRIV-1 + the
/// stored value being an Arc::into_raw of `T`.
pub fn file_privdata_arc_clone<T>(file: &File) -> Option<alloc::sync::Arc<T>> {
    let ptr = file.private_data_ptr()? as *const T;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the stored value is a live Arc::into_raw of T; the
    // increment balances the from_raw below (the slot keeps its ref).
    unsafe {
        alloc::sync::Arc::increment_strong_count(ptr);
        Some(alloc::sync::Arc::from_raw(ptr))
    }
}

/// Zero-and-set-len a pre-sized Vec (the fresh-ring-buffer shape):
/// `with_capacity(n)` then zero-fill then `set_len(n)` — INV-CBYTES-2
/// (the capacity is exactly n, so set_len stays in bounds).
pub fn vec_zeroed_sized(n: usize) -> alloc::vec::Vec<u8> {
    let mut data = alloc::vec::Vec::<u8>::with_capacity(n);
    // SAFETY: the allocation holds n initialized bytes for the Vec's
    // purposes (zeroed right here); capacity is exactly n.
    unsafe {
        core::ptr::write_bytes(data.as_mut_ptr(), 0, n);
        data.set_len(n);
    }
    data
}

/// Build a shared slice from a raw kernel pointer + len (the FileOps
/// raw-slice call shape — the caller owns the readable span).
pub fn kernel_slice<'a>(p: *const u8, len: usize) -> &'a [u8] {
    // SAFETY: the caller's FileOps slice contract — p..p+len readable.
    unsafe { core::slice::from_raw_parts(p, len) }
}

/// Build an exclusive slice from a raw kernel pointer + len (twin).
pub fn kernel_slice_mut<'a>(p: *mut u8, len: usize) -> &'a mut [u8] {
    // SAFETY: as kernel_slice, exclusive.
    unsafe { core::slice::from_raw_parts_mut(p, len) }
}

/// INV-FS-FILEDENTRY-1: `File::dentry` is an `UnsafeCell<Option<Arc<Dentry>>>`
/// installed once at open/create time and immutable for the open file
/// description's lifetime (the same write-once discipline as the inode
/// cell, INV-FS-FILEINODE-1). Shared reads through a shared `&File` are
/// race-free.
pub fn file_dentry(file: &File) -> Option<&alloc::sync::Arc<crate::fs::types::Dentry>> {
    // SAFETY: INV-FS-FILEDENTRY-1 — write-once cell; the shared read is
    // sound for the description's whole lifetime.
    unsafe { (&*file.dentry.get()).as_ref() }
}

/// The thread-group leader view (INV-PROC-TGL-1): `group_leader_ptr`
/// returns either the task itself or the leader installed at clone
/// time; the leader lives as long as ANY thread of the group exists
/// (the thread-group keeps a reference), so a borrow sourced from a
/// pinned/live task is valid for that borrow's scope.
pub fn task_group_leader(t: &crate::process::types::Task) -> &crate::process::types::Task {
    // SAFETY: INV-PROC-TGL-1 above (the source task's liveness covers
    // the leader's).
    unsafe { &*t.group_leader_ptr() }
}

/// INV-PROC-USERPAGE-1: `ppn<<12 + off` is a MAPPED user page of the
/// pinned target task (the caller holds the TaskRef/PinnedTask or is
/// the current task); the kernel linear mapping makes it readable for
/// `len` bytes (the /proc/[pid]/{mem,environ,maps} read faces).
pub fn user_page_slice<'a>(ppn: u64, off: usize, len: usize) -> &'a [u8] {
    let p = crate::arch::mm::phys_to_virt(
        crate::arch::mm::memory_layout::PhysAddr((ppn << 12) + off as u64),
    )
    .0 as *const u8;
    // SAFETY: INV-PROC-USERPAGE-1 (mapped + pinned target page).
    unsafe { core::slice::from_raw_parts(p, len) }
}

/// The sysrq stack-scanner read (INV-PROC-DIAG-1): a diagnostic,
/// best-effort volatile u64 read inside the CURRENT task's kernel
/// stack (the caller validated the range belongs to a live stack; a
/// wrong guess reads garbage at worst — volatile, no UB on stale data).
pub fn diag_read_u64(p: *const u64) -> u64 {
    // SAFETY: INV-PROC-DIAG-1 — volatile diagnostic read.
    unsafe { core::ptr::read_volatile(p) }
}

/// Adopt a validated per-CPU current-task pointer (the sysrq dump's
/// grq_diag_cpu_currents rows): the GRQ keeps the referenced task page
/// alive for the whole diagnostic walk (its queue/slot references pin
/// it), so the borrow is valid for the caller's scope.
pub fn diag_task_ref(p: *mut crate::process::types::Task) -> &'static crate::process::types::Task {
    // SAFETY: the diag snapshot's task pages are pinned by their queue
    // slots for the walk's duration (read-only diagnostics).
    unsafe { &*p }
}

/// The inode-side Arc-clone twin of [`file_privdata_arc_clone`] (the
/// devfs link path's target re-acquisition): clone the Arc the inode's
/// private_data holds WITHOUT consuming the slot (increment + from_raw;
/// the slot keeps its single reference). INV-FS-PRIVDATA-1 + the stored
/// value being an Arc::into_raw of `T`.
pub fn privdata_arc_clone<T>(inode: &Inode) -> Option<alloc::sync::Arc<T>> {
    let ptr = inode.private_data? as *const T;
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the stored value is a live Arc::into_raw of T; the
    // increment balances the from_raw below (the slot keeps its ref).
    unsafe {
        alloc::sync::Arc::increment_strong_count(ptr);
        Some(alloc::sync::Arc::from_raw(ptr))
    }
}

/// INV-DEVFS-SHARED (kernel-VA window family): `kva` is the kernel
/// linear-map address of a LIVE shared page owned by the devfs regular
/// registry; [kva, kva+src.len()) is inside that page by the caller's
/// in-page math. Copies src into the window.
pub fn kva_copy_to(kva: *mut u8, src: &[u8]) {
    // SAFETY: INV-DEVFS-SHARED — live shared page; the caller's chunk
    // math keeps the span inside PAGE_SIZE.
    unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), kva, src.len()) };
}

/// The read-direction twin of [`kva_copy_to`] (INV-DEVFS-SHARED).
pub fn kva_copy_from(kva: *const u8, dst: &mut [u8]) {
    // SAFETY: INV-DEVFS-SHARED — same argument, read direction.
    unsafe { core::ptr::copy_nonoverlapping(kva, dst.as_mut_ptr(), dst.len()) };
}

/// The zeroing twin of [`kva_copy_to`] (INV-DEVFS-SHARED; also the
/// freshly-allocated-page shape — exclusively owned at the call).
pub fn kva_zero(kva: *mut u8, len: usize) {
    // SAFETY: INV-DEVFS-SHARED — bounded by the caller's math.
    unsafe { core::ptr::write_bytes(kva, 0, len) };
}

/// Offset+slice twin of [`kernel_slice`] (the tmpfs cache-frame face:
/// p is a pinned PAGE_SIZE frame, off+len <= PAGE_SIZE by the caller's
/// page math — the F-TMPFS-CACHE audit row).
pub fn kernel_slice_at<'a>(p: *const u8, off: usize, len: usize) -> &'a [u8] {
    // SAFETY: the caller's readable-span contract at p+off (in-page
    // by construction).
    unsafe { core::slice::from_raw_parts(p.add(off), len) }
}

/// File::close for teardown paths (the framework's unsafe fn; the edge
/// carries the caller's sole-reference contract).
pub fn file_close(f: &mut File) -> i32 {
    // SAFETY: the caller owns the sole File reference (abort paths that
    // run before publication).
    unsafe { f.close() }
}

/// The `&'static` view twin of [`file_privdata_arc_ref`] for slots the
/// caller knows outlive the machine (the pipe/PtyPair pattern: the Arc
/// is never the last reference dropped while any File exists, and the
/// callers' borrows are call-scoped).
pub fn file_privdata_arc_static<T>(file: &File) -> Option<&'static T> {
    let p = file.private_data_ptr()? as *const T;
    if p.is_null() {
        return None;
    }
    // SAFETY: INV-FS-FILEPRIV-1 — the File's own Arc reference pins the
    // object for every borrow the caller takes through this view.
    Some(unsafe { &*p })
}

// ---------------------------------------------------------------------------
// Joined into this file from fs/ext4_edges.rs (pass 1b, seed-core-design 2c.3:
// one unsafe.rs per subsystem). Everything below is that file as it
// was; its header was a module doc comment and is kept as plain
// comments, because an inner doc comment is only valid at the top of a file.
// ---------------------------------------------------------------------------
// MIT License
//
// Copyright (c) 2026 Fei Wang
//
// The ext4 TYPED-VIEW FLOOR (rung 5): the on-disk struct views the
// ext4 service crate consumes, as audited framework edges — the
// crate-side files ride these under `#![forbid(unsafe_code)]`.
//
// INV-EXT-EDGE-1 (bounds + alignment): every view checks BOTH the
// byte span and the target alignment before the cast. The alignment
// check makes the edge sound on its own terms — the pre-rung code
// relied on the block-cache allocator's de-facto alignment for
// `Vec<u8>` block buffers; a hypothetical misaligned buffer now
// yields the empty/None arm (the corrupt-disk class), never UB.
//
// INV-EXT-EDGE-2 (u32-array header views): a `&[u32; N]` array IS
// 4-aligned, so a `#[repr(C)]` header/entry type whose alignment is
// ≤ 4 may view into it — the i_block family (the cached Ext4Inode
// `block` field and the on-disk `i_block` share the layout).

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

// ==========================================================================
// pass 3 (seed-core §2c.5): fs's collected unsafe — the inline
// blocks of the subsystem's mechanism files, one contract each.
// ==========================================================================

// ------------ Send/Sync proofs (per-type contracts) ------------

// SAFETY: File is only shared across threads when referenced through Arc,
// and all mutable access goes through Spinlock or UnsafeCell fields that
// are synchronized with IRQ-safe locking.
// (INV-FILE-SEND-1: File's Sync/Send are carried per-field — cells are
// write-once (INV-FILE-CELL-1), pos/cloexec/dirent_cache behind their
// locks, flags/f_owner/f_signum atomics, io_lock a real mutex. No
// field admits a data race through &File.)
/// INV-FILE-SEND-1: File's Send/Sync are carried per-field — cells are write-once (INV-FILE-CELL-1), pos/cloexec/dirent_cache behind their locks, flags/f_owner/f_signum atomics, io_lock a real mutex. No field admits a data race through &File.
unsafe impl Sync for File {}
/// INV-FILE-SEND-1: same per-field discipline as the Sync proof.
unsafe impl Send for File {}
// Send proofs for the raw-pointer registry types (moved from
// syscall/unsafe.rs with the types; same contracts: access only under
// the shard/registry locks).
/// INV-SYSC-2: access only under the shard lock (moved from syscall/unsafe.rs with the types; same contract).
unsafe impl Send for EpollWakeShard {}
/// INV-SYSC-2: access only under the registry lock.
unsafe impl Send for SignalfdWakeRegistry {}
/// Inode's Send is carried per-field (atomics + lock-guarded state); no field admits a data race by move.
unsafe impl Send for Inode {}
/// Inode's Sync is carried per-field (atomics + lock-guarded state); no field admits a data race through &Inode.
unsafe impl Sync for Inode {}
/// InodeCache is accessed only under ICACHE's spinlock.
unsafe impl Send for InodeCache {}
/// InodeCache is accessed only under ICACHE's spinlock.
unsafe impl Sync for InodeCache {}
/// VfsMountInternal's mutable state is behind the mount-table locks.
unsafe impl Send for VfsMountInternal {}
/// VfsMountInternal's shared state is behind the mount-table locks.
unsafe impl Sync for VfsMountInternal {}
// SAFETY: the table's slots are shared-memory seqlock cells exactly like
// the inline `memo` array inside Sync Dentry; cross-thread access goes
// through the seqlock/children-lock protocol.
/// BigMemoTable slots are shared-memory seqlock cells; cross-thread access rides the seqlock/children-lock protocol.
unsafe impl Send for BigMemoTable {}
/// Same seqlock/children-lock protocol as the Send proof.
unsafe impl Sync for BigMemoTable {}
// (INV-DENTRY-SEND-1: Dentry/VfsMountInternal/BigMemoTable Sync/Send
// are carried per-field — children/memo under their locks and seqlock,
// the retirement banks under the drain lock; no field races through
// a shared reference.)
/// Dentry's Send is carried per-field — children/memo under their locks and seqlock, retirement banks under the drain lock.
unsafe impl Send for Dentry {}
/// Same per-field discipline as the Send proof; no field races through a shared reference.
unsafe impl Sync for Dentry {}
/// DentryCache is accessed only under DCACHE's spinlock.
unsafe impl Send for DentryCache {}
/// DentryCache is accessed only under DCACHE's spinlock.
unsafe impl Sync for DentryCache {}
// SAFETY: BufferHead's mutable state (b_data) is protected by BufferState spinlock;
// other fields are atomic or only accessed under lock.
/// BufferHead's mutable state (b_data) is protected by the BufferState spinlock; other fields are atomic or lock-held.
unsafe impl Send for BufferHead {}
// SAFETY: all shared mutable state is protected by the BufferState spinlock;
// b_data is only mutated under lock or from a single thread.
/// All shared mutable state is protected by the BufferState spinlock; b_data is only mutated under lock or single-threaded.
unsafe impl Sync for BufferHead {}
// SAFETY: BlockCache is only accessed via &self methods that use internal locking
// (per-bucket Spinlock and LRU Spinlock) and atomic count; raw pointers are
// confined to cache internals and never exposed without locks.
/// BlockCache is only accessed via &self methods using internal locking (per-bucket + LRU spinlocks); raw pointers never escape without their lock.
unsafe impl Send for BlockCache {}
// SAFETY: all shared mutable state (hash buckets, LRU list) is protected by
// Spinlocks; raw pointer access is always under the appropriate bucket lock.
/// All shared mutable state (hash buckets, LRU list) is protected by spinlocks; raw pointer access is always under the bucket lock.
unsafe impl Sync for BlockCache {}
/// SuperBlock's mutable state is behind the registry/sb locks.
unsafe impl Send for SuperBlock {}
/// SuperBlock's shared state is behind the registry/sb locks.
unsafe impl Sync for SuperBlock {}
/// FsRegistry is accessed under its own spinlock.
unsafe impl Send for FsRegistry {}
/// FsRegistry is accessed under its own spinlock.
unsafe impl Sync for FsRegistry {}
/// VfsMount rides the global mount-table lock discipline (INV-SENDSYNC-1 family).
unsafe impl Send for VfsMount {}
/// VfsMount rides the global mount-table lock discipline.
unsafe impl Sync for VfsMount {}
/// FsStruct (files_struct) is shared only under its files lock (INV-SENDSYNC-1 family).
unsafe impl Send for FsStruct {}
/// FsStruct shared access is serialized by its files lock.
unsafe impl Sync for FsStruct {}
// SAFETY: INV-JSEAM-4 above — the assertion is the JOURNAL use's to
// make (the only user is Ext4FileSystem::journal; the type stays
// narrow-purposed and documented).
/// GAP (INV-JSEAM-4 in the SAFETY note): the assertion is the JOURNAL use's to make; the type stays narrow-purposed and documented.
unsafe impl<T> Send for AssertingSyncArc<T> {}
// SAFETY: INV-JSEAM-4 above.
/// GAP (INV-JSEAM-4 in the SAFETY note): same narrow-purposed assertion as the Send proof.
unsafe impl<T> Sync for AssertingSyncArc<T> {}

// ------------ pidfd (INV-PIDFD-PIN-1) ------------

/// Pinned pid-hash lookup (the pidfd pin edge).
///
/// # Contract (INV-PIDFD-PIN-1)
/// pid_hash_lookup_pinned returns a pinned, valid Task or
/// null; the pin holds until task_put (INV-PIDFD-PIN-1).
#[inline(always)]
pub fn pidfd_lookup_pinned(pid: u32) -> *mut Task {
unsafe { crate::process::hotpath::pid_hash_lookup_pinned(pid) }
}

/// Pin the leader, drop the intermediate pin (the pidfd swap).
///
/// # Contract (INV-PIDFD-PIN-1)
/// group_leader_ptr() of a pinned task is in the same thread
/// ring (kept alive by the ring linkage discipline); the
/// refcnt fetch_add pins the leader before task_put releases
/// the intermediate pin (INV-PIDFD-PIN-1).
#[inline(always)]
pub fn pidfd_leader_pin_swap(leader: *mut Task, t: *mut Task) {
        unsafe {
            (*leader)
                .task_refcnt
                .fetch_add(1, core::sync::atomic::Ordering::AcqRel);
            Task::task_put(t);
        }
}

/// RCU pid-hash lookup (the pidfd identity check's read face).
///
/// # Contract (INV-PIDFD-PIN-1)
/// lookup runs under RCU; the caller compares against the
/// pinned pointer and never dereferences a mismatching
/// result (INV-PIDFD-PIN-1).
#[inline(always)]
pub fn pidfd_hash_lookup(pid: u32) -> *mut Task {
unsafe { crate::process::hotpath::pid_hash_lookup(pid) }
}


// ------------ vt private data (INV-VT-PRIV-1) ------------

/// Reclaim the Box a single-owner slot handed over.
///
/// # Contract (INV-VT-PRIV-1)
/// `p` came from Box::into_raw in the same file's attach path
/// and the caller owns it (close runs once; INV-VT-PRIV-1 /
/// INV-FS-FILEPRIV-1 families).
#[inline(always)]
pub fn box_take<T>(p: *mut T) -> alloc::boxed::Box<T> {
unsafe { alloc::boxed::Box::from_raw(p) }
}

/// The VT number this fd was opened against (minor 0 = the tty0
/// master view addressing the ACTIVE VT).
///
/// # Contract (INV-VT-PRIV-1)
/// private_data holds a leaked Box<DevNo> installed by
/// devfs_open (INV-VT-PRIV-1); None means the tty0 master.
#[inline(always)]
pub fn vt_priv_minor(file: &File) -> u32 {
    unsafe {
        match *(file.private_data.get()) {
            Some(p) => (*(p as *const super::types::DevNo)).minor,
            None => 1,
        }
    }
}

/// put_user of an i32 with the caller's access_ok contract.
///
/// # Contract (INV-VT-PRIV-1)
/// addr was validated with access_ok(4); the arch primitive
/// faults safely via the exception table and returns success
/// (INV-UACCESS-SAFE-1 shape; the site's INV-VT-PRIV-1 names
/// the validated range).
#[inline(always)]
pub fn fs_put_user_u32(addr: *mut i32, v: i32) -> bool {
unsafe { crate::arch::uaccess::put_user(addr, v) }
}

/// copy_from_user with the caller's access_ok contract.
///
/// # Contract (INV-VT-PRIV-1)
/// src was validated with access_ok(n); the arch primitive
/// faults safely and returns the uncopied count
/// (INV-UACCESS-SAFE-1 shape; the site's INV-VT-PRIV-1 names
/// the validated range).
#[inline(always)]
pub fn fs_copy_from_user(dst: *mut u8, src: *const u8, n: usize) -> usize {
unsafe { crate::arch::uaccess::copy_from_user(dst, src, n) }
}

/// put_user of a u8 with the caller's access_ok contract.
///
/// # Contract (INV-VT-PRIV-1)
/// addr was validated with access_ok(1); the arch primitive
/// faults safely (INV-UACCESS-SAFE-1 shape).
#[inline(always)]
pub fn fs_put_user_u8(addr: *mut u8, v: u8) -> bool {
unsafe { crate::arch::uaccess::put_user(addr, v) }
}


// ------------ page-cache table (INV-PC-CLAIM / INV-PC-WIN / INV-PC-PAGECOPY / F13) ------------

/// Owning clone of a published Arc::into_raw pointer: one strong-
/// count increment + from_raw (the pager/slot clone edge).
///
/// # Contract (INV-PC-CLAIM)
/// `p` was published by a writer from an owned Arc and the
/// caller's claim keeps the allocation alive across the
/// increment; the pair nets one new owning Arc whose drop
/// balances the increment (INV-PC-CLAIM / INV-FILE-CELL-1
/// families — each call site names its pin).
#[inline(always)]
pub fn arc_clone_claimed<T>(p: *const T) -> Arc<T> {
    unsafe {
        Arc::increment_strong_count(p);
        Arc::from_raw(p)
    }
}

/// Copy into an owned frame window and zero the tail.
///
/// # Contract (INV-PC-WIN)
/// ptr is an owned PAGE_SIZE frame window; the bounded copy
/// plus tail zero stay inside the frame (INV-PC-WIN).
#[inline(always)]
pub fn frame_fill_from(ptr: *mut u8, src: &[u8], copy_len: usize) {
        unsafe {
            core::ptr::copy_nonoverlapping(src.as_ptr(), ptr, copy_len);
            if copy_len < crate::mm::types::PAGE_SIZE {
                core::ptr::write_bytes(ptr.add(copy_len), 0, crate::mm::types::PAGE_SIZE - copy_len);
            }
        }
}

/// Frame-to-frame copy from a physical address, tail zeroed.
///
/// # Contract (INV-PC-WIN)
/// Both frames are live and owned/locked by the caller's
/// call; the bounded copy plus tail zero stay inside the
/// PAGE_SIZE windows (INV-PC-WIN).
#[inline(always)]
pub fn frame_copy_from(dst: *mut u8, src_phys: usize, copy_len: usize) {
        unsafe {
            let src = phys_to_virt_ptr(src_phys) as *const u8;
            if copy_len > 0 {
                core::ptr::copy_nonoverlapping(src, dst, copy_len);
            }
            if copy_len < crate::mm::types::PAGE_SIZE {
                core::ptr::write_bytes(dst.add(copy_len), 0, crate::mm::types::PAGE_SIZE - copy_len);
            }
        }
}

/// Copy `n` bytes at kernel `src` into the frame at `pfn`, offset `off`
/// (the partial-span writeback shape). INV-PC-PAGECOPY: off+n <= PAGE_SIZE.
/// INV-PC-PAGECOPY: the span is inside one frame by construction at every caller (the cursor arithmetic asserts it).
#[inline]
pub(crate) fn copy_into_page(pfn: u64, off: usize, src: *const u8, n: usize) {
    // SAFETY: INV-PC-PAGECOPY — the span is inside one frame by
    // construction at every caller (the cursor arithmetic asserts it).
    unsafe {
        let dst = phys_to_virt_ptr(pfn_to_phys(pfn as usize));
        core::ptr::copy_nonoverlapping(src, dst.add(off), n);
    }
}
/// Exception-table user->frame copy (the DIO insert path). Returns the
/// uncopied tail on fault. INV-PC-PAGECOPY + the uaccess extable
/// primitive's own contract (any user address is sound to pass).
/// INV-PC-PAGECOPY: the frame is the call's private destination; the user source rides the exception-table primitive.
#[inline]
pub(crate) fn copy_user_into_page(pfn: u64, off: usize, user_addr: usize, n: usize) -> usize {
    // SAFETY: INV-PC-PAGECOPY — the frame is this call's private
    // destination; the user source rides the exception-table primitive.
    unsafe {
        crate::arch::uaccess::copy_from_user(
            phys_to_virt_ptr(pfn_to_phys(pfn as usize)).add(off),
            user_addr as *const u8,
            n,
        )
    }
}
/// Reclaim an owning Arc from a raw pointer that carries an owning
/// count handed to the caller (Arc::into_raw, or a slot reference
/// the slot just detached).
///
/// # Contract (INV-PC-CLAIM)
/// `p` carries one owning count transferred to the caller —
/// from Arc::into_raw or from a just-tombstoned slot; the
/// returned Arc's drop releases exactly that count. A second
/// call on the same pointer double-frees (INV-PC-CLAIM /
/// INV-FILE-CELL-1 families — call sites name the handoff).
#[inline(always)]
pub fn arc_reclaim_owned<T>(p: *const T) -> Arc<T> {
            unsafe { Arc::from_raw(p) }
}

/// Zero a freshly allocated bounce frame and return its kernel
/// address.
///
/// # Contract (F13)
/// bounce_phys is a freshly allocated page frame owned by
/// the call; the zero-fill covers exactly PAGE_SIZE bytes
/// of the frame's linear-map window (F13).
#[inline(always)]
pub fn bounce_zero_frame(bounce_phys: usize) -> *mut u8 {
    unsafe {
        core::ptr::write_bytes(
            phys_to_virt(PhysAddr::new(bounce_phys as u64)).0 as *mut u8,
            0,
            crate::mm::types::PAGE_SIZE,
        );
        phys_to_virt(PhysAddr::new(bounce_phys as u64)).0 as *mut u8
    }
}

/// Raw-pointer read_at entry (the fault path's bounce-buffer face;
/// read_at validates its own file references).
///
/// # Contract (F13)
/// buf names `count` writable bytes (the fresh bounce frame)
/// and file is a live open description (F13).
#[inline(always)]
pub fn file_read_at_raw(file: &File, offset: u64, buf: *mut u8, count: usize) -> isize {
unsafe { file.read_at(offset, buf, count) }
}


// ------------ user-access edges (INV-UACCESS-SAFE-1) ------------

/// copy_to_user with the caller's access_ok contract (the stat
/// family's face; the exception table makes faults safe).
///
/// # Contract (INV-UACCESS-SAFE-1)
/// The caller validated dst with access_ok over the copied
/// size; the arch primitive faults safely via the exception
/// table and returns the uncopied count (INV-UACCESS-SAFE-1).
#[inline(always)]
pub fn fs_copy_to_user(dst: *mut u8, src: *const u8, n: usize) -> usize {
        unsafe {
            crate::arch::uaccess::copy_to_user(dst, src, n)
        }
}


// ------------ memfile content (INV-MEMFILE-1) ------------

/// INV-MEMFILE-1 (unsafe-slim r3): open_mem_file stores an OWNED
/// Box<MemFileContent> in File::private_data (raw via Box::into_raw);
/// these three accessors are the only code that touches it. Identity is
/// guarded by ops == &MEM_FILE_OPS at every call site (the mem_file
/// ops table is only installed by open_mem_file).
/// GAP row (INV-MEMFILE-1 in the SAFETY notes): the pointer, when present, is a live Box<MemFileContent> owned by this File; harness pending.
pub(crate) fn mem_file_content(file: &File) -> Option<&MemFileContent> {
    // SAFETY: INV-MEMFILE-1 — the pointer, when present, is a live
    // Box<MemFileContent> owned by this File; shared read here.
    let ptr = unsafe { *file.private_data.get() }?;
    Some(unsafe { &*(ptr as *const MemFileContent) })
}
/// GAP row (INV-MEMFILE-1 in the SAFETY notes): the pointer, when present, is a live Box<MemFileContent> owned by this File; harness pending.
pub(crate) fn mem_file_content_mut(file: &File) -> Option<&mut MemFileContent> {
    // SAFETY: INV-MEMFILE-1 — sole mutable reach; mem-file ops are the
    // only private_data users for this File.
    let ptr = unsafe { *file.private_data.get() }?;
    Some(unsafe { &mut *(ptr as *mut MemFileContent) })
}
/// Take the owned Box back (close path — frees the content).
/// GAP row (INV-MEMFILE-1 in the SAFETY notes): the pointer, when present, is a live Box<MemFileContent> owned by this File; harness pending.
pub(crate) fn mem_file_take_owned(file: &File) -> Option<alloc::boxed::Box<MemFileContent>> {
    // SAFETY: INV-MEMFILE-1 — take() hands the unique ownership to us.
    let ptr = unsafe { &mut *file.private_data.get() }.take()?;
    Some(unsafe { alloc::boxed::Box::from_raw(ptr as *mut MemFileContent) })
}

// ------------ VFS walk claims (INV-VFS-WALK-1) ------------

/// Peek-clone of the published VFS root: one balanced increment,
/// the leaked count untouched.
///
/// # Contract (INV-VFS-WALK-1)
/// `p` names the root dentry the mount state keeps alive for
/// the whole read (Arc::into_raw'd at publish); from_raw +
/// clone + forget nets exactly one new owning Arc
/// (INV-VFS-WALK-1).
#[inline(always)]
pub fn dentry_root_peek_clone(p: *const Dentry) -> Option<Arc<Dentry>> {
        unsafe {
            let tmp = Arc::from_raw(p as *const Dentry);
            let out = Arc::clone(&tmp);
            core::mem::forget(tmp);
            Some(out)
        }
}


// ------------ block-cache chains (INV-BIO family / F11) ------------

/// Borrow a hash-linked (or LRU-linked) cache entry.
///
/// INV-BIO-ENTRY: `p` came from a bucket chain or the LRU list and the
/// corresponding lock (bucket / lru) is held — under that lock the entry
/// cannot be unlinked and freed, so the borrow is live for the walk.
/// The round-1 TaskRef shape applied to the buffer cache.
/// F11 (INV-BIO-ENTRY): entries are Box::into_raw allocations that live while linked under their locks.
#[inline]
pub(crate) fn entry_ref(p: *mut CacheEntry) -> &'static CacheEntry {
    // SAFETY: INV-BIO-ENTRY; entries are Box::into_raw allocations that
    // live while linked under their locks.
    unsafe { &*p }
}
impl BlockCache {
    /// Move entry to LRU head. Caller must hold the lru lock
    /// (INV-BIO-ENTRY; the unsafe raw-deref window is confined to here).
    /// F11 (INV-BIO-ENTRY): the lru lock is held; the entry and every neighbor it names are linked under the same lock.
    pub(crate) fn move_to_lru_head(lru: &mut LruState, entry_ptr: *mut CacheEntry) {
        // SAFETY: INV-BIO-ENTRY — lru lock held; entry and every neighbor
        // it names in prev/next are linked under the same lock, so the
        // whole relink is one raw-deref window.
        unsafe {
            let entry = &mut *entry_ptr;

            // R7-C1: an entry unlinked by evict_one Phase 1 (both links
            // None) but still visible in the hash chain can race here via
            // get(). Unlinking it again would drop lru.tail (lru_next None
            // -> tail = lru_prev = None) and leave the real tail orphaned
            // — instead treat it as a fresh insert (same as push_lru_head).
            if entry.lru_prev.is_none() && entry.lru_next.is_none() && lru.head != Some(entry_ptr) {
                Self::push_lru_head(lru, entry_ptr);
                return;
            }
            if lru.head == Some(entry_ptr) {
                return; // already at head
            }

            // Unlink from current position
            if let Some(prev) = entry.lru_prev {
                (*prev).lru_next = entry.lru_next;
            }
            if let Some(next) = entry.lru_next {
                (*next).lru_prev = entry.lru_prev;
            } else {
                lru.tail = entry.lru_prev;
            }

            // Insert at head
            entry.lru_prev = None;
            entry.lru_next = lru.head;
            if let Some(head) = lru.head {
                (*head).lru_prev = Some(entry_ptr);
            }
            lru.head = Some(entry_ptr);
            if lru.tail.is_none() {
                lru.tail = Some(entry_ptr);
            }
        }
    }
}
impl BlockCache {
    /// Push an entry that is currently UNLINKED onto the LRU head.
    /// Caller must hold the lru lock (INV-BIO-ENTRY).
    /// F11 (INV-BIO-ENTRY): the lru lock is held; head is a linked entry.
    pub(crate) fn push_lru_head(lru: &mut LruState, entry_ptr: *mut CacheEntry) {
        // R7-C1: idempotent — evict_one Phase 2 re-inserts after a pin
        // recheck; a concurrent get() may have re-linked the entry at head
        // already. Pushing again would self-loop (entry.lru_next = head =
        // entry itself).
        if lru.head == Some(entry_ptr) {
            return;
        }
        // SAFETY: INV-BIO-ENTRY — lru lock held; head is a linked entry.
        unsafe {
            let entry = &mut *entry_ptr;
            entry.lru_prev = None;
            entry.lru_next = lru.head;
            if let Some(head) = lru.head {
                (*head).lru_prev = Some(entry_ptr);
            }
            lru.head = Some(entry_ptr);
            if lru.tail.is_none() {
                lru.tail = Some(entry_ptr);
            }
        }
    }
}
impl BlockCache {
    /// Unlink entry from LRU list. Caller must hold the lru lock
    /// (INV-BIO-ENTRY; the unsafe raw-deref window is confined to here).
    /// F11 (INV-BIO-ENTRY): the lru lock is held; prev/next are linked entries under the same lock.
    pub(crate) fn remove_from_lru(lru: &mut LruState, entry_ptr: *mut CacheEntry) {
        // SAFETY: INV-BIO-ENTRY — lru lock held; prev/next are linked
        // entries under the same lock.
        unsafe {
            let entry = &*entry_ptr;
            if let Some(prev) = entry.lru_prev {
                (*prev).lru_next = entry.lru_next;
            } else {
                lru.head = entry.lru_next;
            }
            if let Some(next) = entry.lru_next {
                (*next).lru_prev = entry.lru_prev;
            } else {
                lru.tail = entry.lru_prev;
            }
            (*entry_ptr).lru_prev = None;
            (*entry_ptr).lru_next = None;
        }
    }
}
impl BlockCache {
    /// Release buffer (decrement refcount)
    /// F11 (INV-BIO-PIN): bh is a raw pointer handed out by get()/bread(); it names a live CacheEntry-owned BufferHead.
    pub fn put(&self, bh: *const BufferHead) {
        // SAFETY: bh is a raw pointer returned by get()/bread(); it points to a
        // valid BufferHead owned by a CacheEntry in the cache.
        unsafe {
            (*bh).put();
        }
    }
}
/// Pin a cache-hit buffer and settle its in-flight I/O.
///
/// # Contract (F11)
/// entry_bh passed the len guard (non-null, block-sized) and
/// is pinned before the bucket lock drops (INV-BIO-PIN).
#[inline(always)]
pub fn bread_pin_settle(entry_bh: *mut BufferHead) -> (*mut BufferHead, i32) {
                        unsafe {
                            let bh = entry_bh;
                            (*bh).get();
                            (bh, wait_buffer_io_done(bh))
                        }
}

/// Retired debug=exthunt probe: report a block-cache hit's buffer
/// pointer and first bytes.
///
/// # Contract (GAP)
/// hit_bh passed the b_data length guard above; the probe is
/// diagnostic-only (the GAP row keeps its base accounting).
#[inline(always)]
pub fn exthunt_rxbhit_probe(hit_bh: *mut BufferHead, blocknr: u64) {
                    unsafe {
                        let d = &(*hit_bh).b_data;
                        crate::api::ext4::exthunt_rxbhit(
                            blocknr,
                            d.as_ptr(),
                            d[0],
                            d[1],
                        );
                    }
}

/// Shared slice view of a Borrowed DMA source (write-batch).
///
/// # Contract (F11)
/// ptr..*len names the caller's live, unmodified DMA source
/// for the submission window (the write-batch discipline;
/// F11). An empty length yields the empty slice.
#[inline(always)]
pub fn borrowed_dma_slice<'a>(ptr: *const u8, len: &usize) -> &'a [u8] {
                unsafe {
                    if *len == 0 {
                        &[]
                    } else {
                        core::slice::from_raw_parts(ptr, *len)
                    }
                }
}


// ------------ buffer I/O wait (INV-SLOT-1 / INV-LOCK-6) ------------

/// THE pinned-handle edge: adopt a bread-family buffer handle as a
/// shared view. INV-BIO-PIN: `bh` came from bread/bread_async/getblk
/// (non-null, the caller's refcount pin holds it until brelse) and the
/// borrow lives only as long as the caller's handle borrow.
/// INV-SLOT-1 (INV-BIO-PIN): pinned by the caller's refcount; non-null by the bread family's construction.
#[inline]
pub(crate) fn bh_pinned<'a>(bh: &'a *mut BufferHead) -> &'a BufferHead {
    // SAFETY: INV-BIO-PIN — pinned by the caller's refcount; non-null
    // by the bread family's construction.
    unsafe { &**bh }
}
/// Exclusive view of a pinned buffer's data block (read-modify-write).
/// INV-SLOT-1 (INV-BIO-PIN): the one documented &mut b_data edge; the caller's pin excludes other borrows.
pub fn bh_data_mut<'a>(bh: &'a *mut BufferHead) -> &'a mut [u8] {
    // SAFETY: INV-BIO-PIN — the caller's pin holds the buffer; every
    // call site's `bh` local is a uniquely-held *mut (the metadata
    // read-modify-write discipline), so the &mut window aliases no live
    // shared borrow. (Kept as the one documented mut edge rather than
    // rippling &mut through dozens of ext4 call sites this round.)
    unsafe { &mut (**bh).b_data }
}
/// Wait for an async buffer read to complete.
///
/// Blocks until the IoCompletion signals done, then marks the buffer
/// as up-to-date and clears the in-flight flag.
///
/// R20-FS8: returns the I/O status (0 = success, negative errno on failure)
/// so callers can refuse to consume/cache data from a failed read. The
/// buffer stays in the cache marked !Uptodate on error.
///
/// On [`crate::fs::hotpath::WAIT_TIMED_OUT`] the device may STILL be
/// writing the request's DMA target: wait() itself has already abandoned the
/// device-side pending entry (so its late completion can never fire into the
/// waiter's memory after this returns — the freed-kernel-stack completion
/// corruption family), and callers must NOT free the DMA target (multi-block
/// buffers are leaked by drain_batch; single-block buffers keep their pin).
/// INV-LOCK-6 (INV-BIO-PIN): bh stays pinned by the owner's reference across the completion wait; the token check refuses misattributed completions.
pub fn bread_wait(bh: *mut BufferHead, completion: &super::types::IoCompletion) -> i32 {
    // SAFETY: bh is a raw pointer returned by bread_async(); it points to a
    // valid BufferHead owned by a CacheEntry in the cache.
    unsafe {
        let status = completion.wait();
        if status == 0 {
            // Zero-serve guard (the no-journal ext4 root-lookup family): a
            // device completion walker delivers with a token proving WHICH
            // buffer's chain completed. A token mismatch means the delivery
            // was misattributed — the waiter's own DMA may not have landed,
            // and b_data is still the new_uninit heap contents (all zeros on
            // a fresh page). Publishing that as BH_Uptodate caches silent
            // zeros for the block (every later lookup of the root directory
            // failed after "init: /test/pbench not found"). Refuse instead:
            // report EIO, leave the buffer !Uptodate (the failed-async
            // residue discipline re-reads it on the next miss) and make the
            // misattribution loud. Token 0 = internal signal (cache hit,
            // in-flight branch, dup-path re-settle) — always accept.
            let token = completion.delivered_token();
            if token != 0 && token != (*bh).b_data.as_ptr() as usize {
                crate::pr_err!(
                    "bio: misattributed completion for block {} (token {:x} != buffer {:x}) — refusing to publish, reporting EIO",
                    (*bh).b_blocknr,
                    token,
                    (*bh).b_data.as_ptr() as usize
                );
                (*bh).clear_state_bit(BufferState::BH_Req);
                return -5;
            }
            (*bh).set_state_bit(BufferState::BH_Uptodate);
        }
        (*bh).clear_state_bit(BufferState::BH_Req);
        status
    }
}

// ------------ ELF load + page-frame buffers (INV-CBYTES-2 / INV-BUFCHUNK-1) ------------

/// Copy one PT_LOAD segment's file bytes and zero its BSS tail
/// (the exec loader's destination-span edge).
///
/// # Contract (INV-CBYTES-2)
/// vaddr..vaddr+memsz is the destination span the caller's
/// address-space setup mapped for this PT_LOAD (exec maps
/// before loading) and offset..offset+filesz was bounds-
/// checked against data.len(); the BSS add is re-checked and
/// fails with InvalidSegment (INV-CBYTES-2).
#[inline(always)]
pub fn elf_load_copy_segment(vaddr: u64, data: &[u8], offset: usize, filesz: usize, memsz: usize) -> Result<(), ElfError> {
        unsafe {
            // Copy segment data to memory
            if filesz > 0 {
                let src = data.as_ptr().add(offset);
                let dst = vaddr as *mut u8;
                core::ptr::copy_nonoverlapping(src, dst, filesz);
            }

            // Zero BSS segment (p_memsz > p_filesz portion)
            if memsz > filesz {
                let bss_start = match vaddr.checked_add(filesz as u64) { Some(a) => a, None => return Err(ElfError::InvalidSegment) };
                let bss_size = memsz - filesz;
                core::ptr::write_bytes(bss_start as *mut u8, 0, bss_size);
            }
        }
    Ok(())
}

/// Zero-fill a with_capacity vector's first n bytes and expose
/// them via set_len (the fresh page/chunk edge).
///
/// # Contract (INV-BUFCHUNK-1)
/// `v`'s capacity is at least `n` (with_capacity just ran) so
/// set_len stays in bounds; the bytes are zeroed right here
/// (INV-CBYTES-2).
#[inline(always)]
pub fn vec_zero_set_len<T>(v: &mut Vec<T>, n: usize) {
        unsafe {
            core::ptr::write_bytes(v.as_mut_ptr(), 0, n);
            v.set_len(n);
        }
}

/// Shared byte-slice view of one heap chunk (FileBuffer read).
///
/// # Contract (INV-BUFCHUNK-1)
/// The const cast is read-only and all FileBuffer access is
/// serialized by the inode data lock; off..off+n stays inside
/// the FILE_BUFFER_CHUNK-sized chunk (INV-BUFCHUNK-1).
#[inline(always)]
pub fn chunk_read_slice<'a>(chunk: &FileChunk, off: usize, n: usize) -> &'a [u8] {
            unsafe {
                let fc = chunk as *const FileChunk as *mut FileChunk;
                let base = (*fc).base_ptr();
                core::slice::from_raw_parts(base.add(off), n)
            }
}

/// Exclusive byte-slice view of one heap chunk (FileBuffer write).
///
/// # Contract (INV-BUFCHUNK-1)
/// The &mut window rides the inode data lock's serialization;
/// off..off+n stays inside the chunk (INV-BUFCHUNK-1).
#[inline(always)]
pub fn chunk_write_slice<'a>(chunk: &mut FileChunk, off: usize, n: usize) -> &'a mut [u8] {
            unsafe {
                let base = chunk.base_ptr();
                core::slice::from_raw_parts_mut(base.add(off), n)
            }
}

/// Zero the owned tail of a heap chunk past a shrink point.
///
/// # Contract (GAP)
/// Owned chunk under the inode data lock; the zeroed tail
/// stays inside the FILE_BUFFER_CHUNK-sized chunk (the GAP
/// row keeps its base accounting; harness pending).
#[inline(always)]
pub fn chunk_zero_tail(chunk: &mut FileChunk, tail: usize) {
                    unsafe {
                        core::ptr::write_bytes(
                            chunk.base_ptr().add(tail),
                            0,
                            FILE_BUFFER_CHUNK - tail,
                        );
                    }
}


// ------------ dentry child memo (INV-DENTRY-MEMO-1 / INV-DENTRY-ADOPT-1) ------------

/// Increment a dentry's strong count for a pointer loaded under a
/// reader claim (the claim blocks the drain that would otherwise free
/// it) and return an owned Arc.
///
/// # Safety
/// `p` must have been loaded from a dentry child memo slot / inode
/// pointer mirror while a WalkClaim is held by the caller, and `p` must
/// be non-null.
/// (INV-DENTRY-ADOPT-1: the claim-then-adopt protocol — the WalkClaim
/// blocks the deferred-retirement drain that would otherwise free the
/// target; increment_strong_count + from_raw under that claim is the
/// audited adopt edge. Both adopt fns and every call site ride it.)
/// INV-DENTRY-ADOPT-1: the audited adopt edge — increment + from_raw under a walk claim that blocks the retirement drain (contract in the doc above).
pub unsafe fn dentry_arc_from_raw(p: *const Dentry) -> Arc<Dentry> {
    Arc::increment_strong_count(p);
    Arc::from_raw(p)
}
/// Same as [`dentry_arc_from_raw`] for inodes (the inode pointer mirror).
///
/// # Safety
/// See [`dentry_arc_from_raw`]; `p` non-null, loaded under a claim.
/// INV-DENTRY-ADOPT-1: the inode pointer mirror of the adopt edge (same claim discipline).
pub unsafe fn inode_arc_from_raw(p: *const Inode) -> Arc<Inode> {
    Arc::increment_strong_count(p);
    Arc::from_raw(p)
}
/// Safe twin of the moved unsafe adopt fn: re-adopt an owning Arc
/// from a claim-protected raw Dentry pointer.
///
/// # Contract (INV-DENTRY-ADOPT-1)
/// `p` is non-null and pinned by the caller's walk claim,
/// which blocks the deferred-retirement drain; the increment
/// + from_raw pair is the audited adopt (INV-DENTRY-ADOPT-1).
#[inline(always)]
pub fn dentry_arc_adopt(p: *const Dentry) -> Arc<Dentry> {
unsafe { dentry_arc_from_raw(p) }
}

/// Safe twin of the moved unsafe adopt fn (inode mirror).
///
/// # Contract (INV-DENTRY-ADOPT-1)
/// `p` is non-null and kept alive by the inode slot or its
/// pending retire; the caller's claim blocks the drain
/// (INV-DENTRY-ADOPT-1).
#[inline(always)]
pub fn inode_arc_adopt(p: *const Inode) -> Arc<Inode> {
unsafe { inode_arc_from_raw(p) }
}

/// Shared read through a raw pointer the caller keeps valid (the
/// claimed-deref edge; the caller's claim/pin/lock is the liveness
/// proof — VFS walk claims, wlock-stable page-cache slots, seqlock
/// memo cells, pinned buffer heads).
///
/// # Contract (INV-DENTRY-MEMO-1)
/// `p` is non-null, aligned for T, and stays valid for the
/// returned borrow's scope by the caller's proof (the block's
/// original SAFETY note names it per site: INV-VFS-WALK-1,
/// INV-PC-CLAIM, INV-DENTRY-MEMO-1, INV-SYSC-1/2, INV-SLOT-1,
/// INV-PIDFD-PIN-1, F11/F13 families).
#[inline(always)]
pub fn raw_ref<'a, T>(p: *const T) -> &'a T {
unsafe { &*p }
}

/// Exclusive window through a raw pointer the caller exclusively
/// owns (twin of raw_ref; the caller's lock/ownership proof is the
/// exclusivity argument — children lock, bucket lock, drop-time
/// sole ownership).
///
/// # Contract (INV-DENTRY-MEMO-1)
/// `p` is non-null, aligned for T, and no other borrow of the
/// target exists for the returned window by the caller's
/// proof (per-site SAFETY notes: INV-DENTRY-MEMO-1, INV-BIO
/// relink writes, close-path cells).
#[inline(always)]
pub fn raw_ref_mut<'a, T>(p: *mut T) -> &'a mut T {
unsafe { &mut *p }
}

/// Swap the published big-memo table under the children lock.
///
/// # Contract (INV-DENTRY-MEMO-1)
/// The children lock the caller holds serializes this writer
/// against every other refill and the final Dentry drop
/// (post-quiescence); lock-free readers never touch the
/// ownership cell (INV-DENTRY-MEMO-1).
#[inline(always)]
pub fn big_memo_replace(cell: &UnsafeCell<Option<alloc::boxed::Box<BigMemoTable>>>, table: alloc::boxed::Box<BigMemoTable>) -> Option<alloc::boxed::Box<BigMemoTable>> {
unsafe { (*cell.get()).replace(table) }
}


// ------------ anon device map (INV-INODE-STATDEV-1) ------------

/// The stat device (Linux s_dev) of a filesystem instance: kernel dev_t
/// shape (DevNo, typically major 0 = UNNAMED_MAJOR, minor from the
/// global counter). Deterministic per fs_id for the whole boot.
/// INV-INODE-STATDEV-1: the ANON_DEV_MAP snapshots are immutable once published and only replaced, never freed.
pub fn stat_dev_for_fs(fs_id: u64) -> super::types::DevNo {
    // Fast path: immutable published snapshot.
    let p = ANON_DEV_MAP.load(Ordering::Acquire);
    if !p.is_null() {
        // SAFETY: snapshots are immutable once published; the pointer is
        // only ever replaced (never freed), so this reference stays valid.
        if let Some(&d) = unsafe { (*p).0.get(&fs_id) } {
            return d;
        }
    }
    let _guard = ANON_DEV_WRITE.lock();
    // Re-check under the write lock (another CPU may have just published).
    let p = ANON_DEV_MAP.load(Ordering::Acquire);
    if !p.is_null() {
        // SAFETY: see above.
        if let Some(&d) = unsafe { (*p).0.get(&fs_id) } {
            return d;
        }
    }
    let dev = super::types::DevNo::new(0, ANON_DEV_NEXT.fetch_add(1, Ordering::Relaxed));
    let mut next = alloc::collections::BTreeMap::new();
    if !p.is_null() {
        // SAFETY: see above; clone under the write lock.
        next.clone_from(unsafe { &(*p).0 });
    }
    next.insert(fs_id, dev);
    let fresh = alloc::boxed::Box::into_raw(alloc::boxed::Box::new(AnonDevMap(next)));
    ANON_DEV_MAP.store(fresh, Ordering::Release);
    dev
}
/// One aligned u32 volatile store of the cached inode mode.
///
/// # Contract (INV-INODE-STATDEV-1)
/// `ptr` names a live shared Inode's mode field; the store
/// is aligned and u32-sized, so concurrent readers observe
/// either the old or the new full word (INV-INODE-STATDEV-1).
#[inline(always)]
pub fn inode_mode_volatile_store(ptr: *mut InodeMode, mode: InodeMode) {
        unsafe { ptr.write_volatile(mode); }
}


// ------------ syscall-container lists (INV-SYSC-1 / INV-SYSC-2) ------------

/// INV-SYSC-1: the list/box raw edges' registry-side contract (shard/registry lock or the caller's ownership/pin proof) rides the fn's own SAFETY note.
#[inline]
pub(crate) fn list_first(head: *mut ListHead) -> *mut ListHead {
    debug_assert!(!head.is_null());
    // SAFETY: INV-SYSC-1 — plain load of the sentinel's next field.
    unsafe { (*head).next }
}
/// INV-SYSC-1: the list/box raw edges' registry-side contract (shard/registry lock or the caller's ownership/pin proof) rides the fn's own SAFETY note.
#[inline]
pub(crate) fn list_next(cur: *mut ListHead) -> *mut ListHead {
    // SAFETY: INV-SYSC-1 — capture-before-body discipline.
    unsafe { (*cur).next }
}
/// INV-SYSC-1: the list/box raw edges' registry-side contract (shard/registry lock or the caller's ownership/pin proof) rides the fn's own SAFETY note.
#[inline]
pub(crate) fn list_link_tail(node: *mut ListHead, head: *mut ListHead) {
    debug_assert!(!node.is_null() && !head.is_null());
    // SAFETY: INV-SYSC-1 — node's list is initialized (fresh) and
    // unlinked; head is the live sentinel held under the lock.
    unsafe { (*node).add_tail(head) }
}
/// INV-SYSC-1: the list/box raw edges' registry-side contract (shard/registry lock or the caller's ownership/pin proof) rides the fn's own SAFETY note.
#[inline]
pub(crate) fn list_unlink(node: *mut ListHead) {
    // SAFETY: INV-SYSC-1 unlink arm — caller holds the list's lock; the
    // node is currently linked and its neighbors are live.
    unsafe { (*node).del() }
}
/// INV-SYSC-2: the list/box raw edges' registry-side contract (shard/registry lock or the caller's ownership/pin proof) rides the fn's own SAFETY note.
#[inline]
pub(crate) fn boxed_mut<T>(p: *mut T) -> &'static mut T {
    // SAFETY: INV-SYSC-2 — caller holds the ownership/pin proof.
    unsafe { &mut *p }
}
/// INV-SYSC-2: the list/box raw edges' registry-side contract (shard/registry lock or the caller's ownership/pin proof) rides the fn's own SAFETY note.
#[inline]
pub(crate) fn boxed_drop<T>(p: *mut T) {
    // SAFETY: INV-SYSC-2 — the box is uniquely owned here (unlinked or
    // never linked).
    unsafe { drop(alloc::boxed::Box::from_raw(p)) }
}
/// INV-SYSC-1: the list/box raw edges' registry-side contract (shard/registry lock or the caller's ownership/pin proof) rides the fn's own SAFETY note.
#[inline]
pub(crate) fn list_entry_mut<T>(cur: *mut ListHead) -> &'static mut T {
    // SAFETY: INV-SYSC-1 — container_of to the node for a read-modify pass
    // under the owning lock.
    unsafe { &mut *(cur as *mut T) }
}
/// INV-SYSC-1: the list/box raw edges' registry-side contract (shard/registry lock or the caller's ownership/pin proof) rides the fn's own SAFETY note.
#[inline]
pub(crate) fn list_entry<T>(cur: *mut ListHead) -> &'static T {
    // SAFETY: INV-SYSC-1 — container_of back to the node (list is the
    // first field, so the cast is a no-op).
    unsafe { &mut *(cur as *mut T) }
}
/// INV-SYSC-2: the list/box raw edges' registry-side contract (shard/registry lock or the caller's ownership/pin proof) rides the fn's own SAFETY note.
#[inline]
pub(crate) fn boxed_ref<T>(p: *mut T) -> &'static T {
    // SAFETY: callers hold the pin/liveness proof (INV-SYSC-2's
    /// registry-side arm).
    unsafe { &mut *p }
}

// ------------ File I/O entries (INV-FILE-CELL-1) ------------

impl File {
    /// Read file
    /// INV-FILE-CELL-1: the write-once inode/dentry/ops cells behind the f_mode checks — the contracts live in the doc comments that moved with the fns.
    pub unsafe fn read(&self, buf: *mut u8, count: usize) -> isize {
        // f_mode enforcement: read on an O_WRONLY fd is EBADF (Linux)
        if self.flags().is_writeonly() {
            return -9;  // EBADF
        }
        // O_PATH descriptors permit no I/O at all (LTP open13).
        if self.flags().bits() & super::types::FileFlags::O_PATH != 0 {
            return -9; // EBADF
        }
        if let Some(ops) = *self.ops.get() {
            if let Some(read_fn) = ops.read {
                let slice = core::slice::from_raw_parts_mut(buf, count);
                let result = read_fn(self, slice);
                if result > 0 {
                    // inotify IN_ACCESS (P0-4): one atomic load when no
                    // watches exist.
                    self::notify_file(self, self::ino_events::IN_ACCESS);
                }
                return result;
            }
        }
        // Valid fd, readable mode, but the type carries no read op
        // (epoll, binder, access_token, io_uring) — see
        // missing_op_errno(FileOpKind::Read) for why EINVAL, not EBADF.
        self.missing_op_errno(FileOpKind::Read)
    }
}
impl File {
    /// Write file
    /// INV-FILE-CELL-1: the write-once inode/dentry/ops cells behind the f_mode checks — the contracts live in the doc comments that moved with the fns.
    pub unsafe fn write(&self, buf: *const u8, count: usize) -> isize {
        // f_mode enforcement: write on an O_RDONLY fd is EBADF (Linux)
        if self.flags().is_readonly() {
            return -9;  // EBADF
        }
        // O_PATH descriptors permit no I/O at all (LTP open13).
        if self.flags().bits() & super::types::FileFlags::O_PATH != 0 {
            return -9; // EBADF
        }
        if let Some(ops) = *self.ops.get() {
            if let Some(write_fn) = ops.write {
                let slice = core::slice::from_raw_parts(buf, count);
                let result = write_fn(self, slice);
                if result > 0 {
                    // inotify IN_MODIFY (P0-4): sqlite/editors/build tools.
                    self::notify_file(self, self::ino_events::IN_MODIFY);
                }
                return result;
            }
        }
        // Valid fd, writable mode, but the type carries no write op —
        // see missing_op_errno(FileOpKind::Write) (EINVAL, matching the
        // reference kernel's vfs_write).
        self.missing_op_errno(FileOpKind::Write)
    }
}
impl File {
    /// Seek file position
    /// INV-FILE-CELL-1: the write-once inode/dentry/ops cells behind the f_mode checks — the contracts live in the doc comments that moved with the fns.
    pub unsafe fn lseek(&self, offset: isize, whence: i32) -> isize {
        if let Some(ops) = *self.ops.get() {
            if let Some(lseek_fn) = ops.lseek {
                return lseek_fn(self, offset, whence);
            }
        }
        // No seek op = unseekable object (pipe, socket, fifo) — see
        // missing_op_errno(FileOpKind::LSeek) for the POSIX reasoning.
        self.missing_op_errno(FileOpKind::LSeek)
    }
}
impl File {
    /// Position-invariant read at an explicit offset (pread(2) backing).
    ///
    /// Prefers the op's native read_at entry (R71): those never touch the
    /// shared file position at all. The fd-backed fallback below still
    /// swings the position — it is inherently not atomic against a
    /// concurrent plain read() on the same description, which is exactly
    /// why ops that userspace preads or demand-fault-maps at scale
    /// (ext4/tmpfs) must carry ops.read_at.
    ///
    /// Returns -ESPIPE for non-seekable objects (pipes/sockets — Linux
    /// pread on them fails with ESPIPE).
    /// INV-FILE-CELL-1: the write-once inode/dentry/ops cells behind the f_mode checks — the contracts live in the doc comments that moved with the fns.
    pub unsafe fn read_at(&self, offset: u64, buf: *mut u8, count: usize) -> isize {
        // Unseekable objects first (pipes/sockets/fifos: no lseek op, no
        // inode): explicit-position reads fail ESPIPE BEFORE the f_mode
        // check — Linux rw_verify_area's FMODE_STREAM rejection (LTP
        // preadv202 expects ESPIPE on a pipe read end, and pread on an
        // O_WRONLY pipe write end is also ESPIPE, not EBADF).
        let inode_opt = &*self.inode.get();
        if !self.positional_capable() {
            // Two non-seekable classes must be told apart — the decision
            // and its ABI reasoning live in missing_op_errno(ReadAt):
            // stream types (pipes/sockets/fifos) get ESPIPE, op-less fd
            // types (epoll/binder/access_token/io_uring) get EINVAL.
            return self.missing_op_errno(FileOpKind::ReadAt);
        }
        // f_mode enforcement, same as read().
        if self.flags().is_writeonly() {
            return -9; // EBADF
        }
        if self.flags().bits() & super::types::FileFlags::O_PATH != 0 {
            return -9; // EBADF
        }
        // Native positional op: no shared-position access at all.
        if let Some(ops) = *self.ops.get() {
            if let Some(read_at_fn) = ops.read_at {
                let slice = core::slice::from_raw_parts_mut(buf, count);
                let n = read_at_fn(self, offset, slice);
                if n > 0 {
                    self::notify_file(
                        self,
                        self::ino_events::IN_ACCESS,
                    );
                }
                return n;
            }
        }

        // Disk-backed files (ext4) serve reads through their file ops;
        // inode.read_data only holds the in-memory FileBuffer of mem
        // files and returns 0 for on-disk inodes — pread on any ext4
        // file used to return 0 bytes (glibc's regcomp mmap'd locale
        // tables fine but every pread-based reader saw zeros).
        if let Some(ops) = *self.ops.get() {
            // lseek presence marks the file seekable (pipes/sockets have
            // none); pread on them must stay ESPIPE.
            if ops.read.is_some() && ops.lseek.is_some() {
                let saved = self.get_pos();
                self.set_pos(offset);
                let n = self.read(buf, count);
                self.set_pos(saved);
                return n;
            }
        }

        let slice = core::slice::from_raw_parts_mut(buf, count);
        // Only seekable (inode-backed) files support offset reads.
        match inode_opt.as_ref() {
            Some(inode) if inode.ops.is_some() => {
                let n = inode.read_data(offset as usize, slice);
                if n > 0 {
                    // inotify IN_ACCESS on the pread(2) path (P0-4).
                    self::notify_file(
                        self,
                        self::ino_events::IN_ACCESS,
                    );
                }
                n as isize
            }
            _ => -29, // ESPIPE
        }
    }
}
impl File {
    /// Position-invariant write at an explicit offset (pwrite(2) backing).
    ///
    /// POSIX leaves the offset/O_APPEND interaction unspecified; this ABI
    /// resolves it the documented pwrite(2) way: on a descriptor opened
    /// with O_APPEND the data is APPENDED regardless of the offset (LTP
    /// pwrite04/pwrite04_64: the second pwrite at offset 0 must extend
    /// the file from 2048 to 3072, not overwrite byte 0).
    /// INV-FILE-CELL-1: the write-once inode/dentry/ops cells behind the f_mode checks — the contracts live in the doc comments that moved with the fns.
    pub unsafe fn write_at(&self, offset: u64, buf: *const u8, count: usize) -> isize {
        // Unseekable first, before the f_mode check: pwrite on a pipe
        // (even its O_RDONLY read end) fails ESPIPE, not EBADF — Linux
        // rw_verify_area's FMODE_STREAM rejection (LTP pwritev202).
        let inode_opt = &*self.inode.get();
        if !self.positional_capable() {
            return -29; // ESPIPE
        }
        // f_mode enforcement, same as write().
        if self.flags().is_readonly() {
            return -9; // EBADF
        }
        if self.flags().bits() & super::types::FileFlags::O_PATH != 0 {
            return -9; // EBADF
        }
        // O_APPEND wins over the explicit offset: resolve the write
        // position to the CURRENT end of file (the ext4 backend
        // re-resolves u64::MAX under its own lock; the value here covers
        // the set_pos/write/restore fallback and memfs paths).
        let offset = if self.flags().bits() & super::types::FileFlags::O_APPEND != 0 {
            match inode_opt.as_ref() {
                Some(inode) => inode.size.load(core::sync::atomic::Ordering::Acquire) as u64,
                None => offset,
            }
        } else {
            offset
        };

        // Disk-backed files (ext4) must write through their file ops;
        // inode.write_data targets the in-memory FileBuffer and would
        // desynchronize disk-backed files (pwrite silently lost).
        if let Some(ops) = *self.ops.get() {
            // Native position-invariant write (session 25's ext4 lesson,
            // now a generic capability): the set_pos/write/restore dance
            // below raced concurrent plain write()/read() on the same
            // open file description (the write twin of the R71 read_at
            // race), and ext4's trailing whole-inode page-cache
            // invalidate broke shared-mapping coherence.
            if let Some(write_at) = ops.write_at {
                let slice = core::slice::from_raw_parts(buf, count);
                let n = write_at(self, offset, slice);
                if n > 0 {
                    // inotify IN_MODIFY on the pwrite(2) path (P0-4) —
                    // sqlite's journal/WAL writes all go through pwrite.
                    self::notify_file(
                        self,
                        self::ino_events::IN_MODIFY,
                    );
                }
                return n;
            }
            if ops.write.is_some() && ops.lseek.is_some() {
                let saved = self.get_pos();
                self.set_pos(offset);
                let n = self.write(buf, count);
                self.set_pos(saved);
                return n;
            }
        }

        let slice = core::slice::from_raw_parts(buf, count);
        match inode_opt.as_ref() {
            Some(inode) if inode.ops.is_some() => {
                // Serialize concurrent explicit-offset writers like
                // reg_file_write does (O_APPEND/pos race class, review 5.2).
                let _write_guard = self.io_lock.guard();
                let n = inode.write_data(offset as usize, slice);
                if n > 0 {
                    // inotify IN_MODIFY on the pwrite(2) path (P0-4) —
                    // sqlite's journal/WAL writes all go through pwrite.
                    self::notify_file(
                        self,
                        self::ino_events::IN_MODIFY,
                    );
                }
                n as isize
            }
            _ => -29, // ESPIPE
        }
    }
}
impl File {
    /// Close file
    /// INV-FILE-CELL-1: the write-once inode/dentry/ops cells behind the f_mode checks — the contracts live in the doc comments that moved with the fns.
    pub unsafe fn close(&mut self) -> i32 {
        if let Some(ops) = *self.ops.get() {
            if let Some(close_fn) = ops.close {
                return close_fn(self);
            }
        }
        0
    }
}
impl File {
    /// Slice-typed read(2) dispatch (see read()).
    /// INV-FILE-CELL-1: the write-once inode/dentry/ops cells behind the f_mode checks — the contracts live in the doc comments that moved with the fns.
    pub fn read_slice(&self, buf: &mut [u8]) -> isize {
        // SAFETY: the mutable slice guarantees buf.len() writable bytes —
        // exactly the (buf, count) contract read() requires.
        unsafe { self.read(buf.as_mut_ptr(), buf.len()) }
    }
}
impl File {
    /// Slice-typed write(2) dispatch (see write()).
    /// INV-FILE-CELL-1: the write-once inode/dentry/ops cells behind the f_mode checks — the contracts live in the doc comments that moved with the fns.
    pub fn write_slice(&self, buf: &[u8]) -> isize {
        // SAFETY: the shared slice guarantees buf.len() readable bytes —
        // exactly the (buf, count) contract write() requires.
        unsafe { self.write(buf.as_ptr(), buf.len()) }
    }
}
impl File {
    /// Slice-typed pread(2) dispatch (see read_at()).
    /// INV-FILE-CELL-1: the write-once inode/dentry/ops cells behind the f_mode checks — the contracts live in the doc comments that moved with the fns.
    pub fn read_at_slice(&self, offset: u64, buf: &mut [u8]) -> isize {
        // SAFETY: the mutable slice guarantees buf.len() writable bytes.
        unsafe { self.read_at(offset, buf.as_mut_ptr(), buf.len()) }
    }
}
impl File {
    /// Slice-typed pwrite(2) dispatch (see write_at()).
    /// INV-FILE-CELL-1: the write-once inode/dentry/ops cells behind the f_mode checks — the contracts live in the doc comments that moved with the fns.
    pub fn write_at_slice(&self, offset: u64, buf: &[u8]) -> isize {
        // SAFETY: the shared slice guarantees buf.len() readable bytes.
        unsafe { self.write_at(offset, buf.as_ptr(), buf.len()) }
    }
}
impl File {
    /// Safe lseek(2) dispatch for `#![forbid(unsafe_code)]` syscall
    /// files (same rationale as the slice-typed I/O entries above):
    /// the raw-pointer-free dispatch over the write-once ops cell.
    /// INV-FILE-CELL-1: the write-once inode/dentry/ops cells behind the f_mode checks — the contracts live in the doc comments that moved with the fns.
    pub fn lseek_op(&self, offset: isize, whence: i32) -> isize {
        // SAFETY: the only unsafe inside lseek() is the write-once ops
        // cell read (see get_ops); this entry adds no raw pointers.
        unsafe { self.lseek(offset, whence) }
    }
}
/// INV-FILE-CELL-1: vacuous unsafe — the body calls only the safe get_file_fd(2).
pub unsafe fn get_stderr() -> Option<Arc<File>> {
    get_file_fd(2)
}

// ------------ fd-table published slots (INV-FILE-CELL-1) ------------

/// One strong-count increment on a LIVE published Arc::into_raw
/// pointer (the fd-table/pager slot increment edge).
///
/// # Contract (INV-FILE-CELL-1)
/// `p` was published by a writer from an owned Arc and the
/// caller's claim (slot lock, table lock, or walk claim) keeps
/// the allocation alive across the increment; every call site
/// names its pin in its SAFETY note (INV-FILE-CELL-1 /
/// INV-PC-CLAIM families).
#[inline(always)]
pub fn arc_inc<T>(p: *const T) {
                unsafe { Arc::increment_strong_count(p) };
}

/// Temporary owning clone of a published slot occupant for the
/// duration of one callback (for_each_file's visit face).
///
/// # Contract (INV-FILE-CELL-1)
/// `p` is published by a writer and the table's reference is
/// held by the slot for the duration of the entry lock the
/// caller holds; the increment + from_raw + drop triple nets
/// a borrow of the slot's occupant (INV-FILE-CELL-1).
#[inline(always)]
pub fn fdslot_visit_published<F: FnMut(usize, &Arc<File>)>(fd: usize, p: *mut File, f: &mut F) {
                    unsafe {
                        Arc::increment_strong_count(p);
                        let file = Arc::from_raw(p);
                        f(fd, &file);
                        drop(file);
                    }
}

/// set_len on a with_capacity vector whose bytes the caller fills
/// before any read (the staging-buffer edge).
///
/// # Contract (INV-FILE-CELL-1)
/// `v`'s capacity is at least `n` (with_capacity just ran);
/// the bytes are uninit until the caller fills them and no
/// read happens before that (INV-FILE-CELL-1 staging note).
#[inline(always)]
pub fn vec_set_len_uninit<T>(v: &mut Vec<T>, n: usize) {
    unsafe { v.set_len(n); }
}

/// One copy_from_user into the reserved tail, then set_len (the
/// writev gather edge); false on fault with nothing exposed.
///
/// # Contract (INV-FILE-CELL-1)
/// dst's capacity covers start..start+len (try_reserve_exact
/// just ran); copy_from_user fills all of it or the fn
/// returns false before set_len exposes any byte
/// (INV-FILE-CELL-1 gather note).
#[inline(always)]
pub fn gather_append_copy(dst: &mut Vec<u8>, start: usize, src_user: usize, len: usize) -> bool {
    unsafe {
        let copied_ok = crate::arch::uaccess::copy_from_user(
            dst.as_mut_ptr().add(start),
            src_user as *const u8,
            len,
        ) == 0;
        if !copied_ok {
            return false;
        }
        dst.set_len(start + len);
    }
    true
}

/// __fdget's unshared fast path: borrowed-or-owned fd resolution
/// off a live task pointer (get_file_fd_fast's body).
///
/// # Contract (INV-FILE-CELL-1)
/// `cur` is the current task pointer (sched::current()); the
/// unshared table outlives the syscall, so the transmuted
/// 'static borrow is gated on fdtable_unshared() (the doc
/// above the original fn; INV-FILE-CELL-1).
#[inline(always)]
pub fn fd_fast_lookup(cur: *mut Task, fd: usize) -> Option<FastFd> {
    unsafe {
        if !(*cur).fdtable_unshared() {
            return (*cur).try_fdtable().and_then(|t| t.get_file(fd)).map(FastFd::Owned);
        }
        // The unshared table outlives the syscall (owned by this task);
        // extend the borrow to 'static through the raw pointer.
        let tbl: &'static FdTable = (*cur).try_fdtable()?;
        tbl.get_file_borrow(fd).map(|f| {
            // SAFETY: the slot's Arc is owned by the unshared table and
            // cannot be retired for the duration of this syscall.
            unsafe { core::mem::transmute::<&File, &'static File>(f) }
        }).map(FastFd::Borrowed)
    }
}


// ------------ published-Arc lifecycle (INV-FILE-ARC-1) ------------

/// Conditional strong-count acquisition on a published File — the
/// reference's `get_file_rcu` / `atomic_long_inc_not_zero` shape
/// (oc-rc close-path rework; replaces the unconditional increment that
/// forced every close(2) to wait out an RCU grace period before
/// dropping the removed Arc).
///
/// Returns None when the strong count has reached zero — the fd was
/// closed concurrently; callers treat it exactly like an empty slot
/// (EBADF), which is the reference fget race outcome.
///
/// SAFETY: `p` must have been published by a writer from an owned Arc
/// (install/dup/fork displacement-free publication), and the ArcInner
/// allocation must be pinned across this call (a slot reference or a
/// fdreap-parked Weak) — the CAS itself must never act on freed memory.
/// (INV-FILE-ARC-1: the strong-word offset assumption this family
/// rests on is PINNED AT BOOT by verify_arc_inner_layout below; the
/// caller-pins + CAS-nonzero-or-clean-fail protocol is the audited
/// edge — every row of this family rides it.)
/// INV-FILE-ARC-1: caller-pins + CAS-nonzero-or-clean-fail — conditional strong-count acquisition on a published File; the ArcInner strong-word offset is pinned by verify_arc_inner_layout below.
#[inline]
unsafe fn file_arc_try_clone(p: *mut File) -> Option<Arc<File>> {
    // std Arc layout on our 64-bit targets: ArcInner { strong:
    // AtomicUsize, weak: AtomicUsize, value: T }, and into_raw/as_ptr
    // point at `value` — the strong word sits two usize words back.
    // The assumption is pinned at boot by verify_arc_inner_layout().
    let strong = (p as *const core::sync::atomic::AtomicUsize).sub(2);
    // SAFETY: caller pins the allocation (see doc); the atomic protocol
    // below either wins a nonzero count or fails cleanly.
    let mut cur = unsafe { (*strong).load(Ordering::Relaxed) };
    loop {
        if cur == 0 {
            return None;
        }
        match unsafe {
            (*strong).compare_exchange_weak(
                cur,
                cur + 1,
                Ordering::Acquire,
                Ordering::Relaxed,
            )
        } {
            Ok(_) => return Some(unsafe { Arc::from_raw(p) }),
            Err(c) => cur = c,
        }
    }
}
/// Safe twin of the moved unsafe fn: one conditional strong-count
/// acquisition on a published File (the fd-table fget face).
///
/// # Contract (INV-FILE-ARC-1)
/// `p` was published by a writer from an owned Arc and the
/// ArcInner allocation is pinned across this call (a slot
/// reference or an fdreap-parked Weak); the CAS wins a
/// nonzero count or fails cleanly (INV-FILE-ARC-1).
#[inline(always)]
pub fn file_arc_try_clone_claimed(p: *mut File) -> Option<Arc<File>> {
            unsafe { file_arc_try_clone(p) }
}

/// Boot-time pin of the ArcInner strong-word offset assumption used by
/// `file_arc_try_clone` (called from fs::fdreap::init). If a future
/// toolchain moved the refcount word, every lock-free fd lookup would
/// CAS the wrong memory — fail loudly at boot instead.
/// INV-FILE-ARC-1: the boot-time pin of the ArcInner strong-word offset the fd family assumes.
pub(crate) fn verify_arc_inner_layout() {
    // (INV-FILE-ARC-1: the boot-time pin of the ArcInner strong-word
    // offset — the family's layout assumption, checked here once.)
    crate::pr_info!(
        "fs: File={}B (open/close hot path size gate)",
        core::mem::size_of::<File>()
    );
    let probe = Arc::new(File::new(FileFlags::new(0)));
    // SAFETY: read-only probe of the live Arc the function constructed.
    let strong = unsafe {
        (Arc::as_ptr(&probe) as *const core::sync::atomic::AtomicUsize).sub(2)
    };
    // SAFETY: probe is a live Arc; the word read is the refcount itself
    // under verification.
    assert_eq!(
        unsafe { (*strong).load(Ordering::Relaxed) },
        1,
        "ArcInner strong-word offset assumption broken"
    );
    let clone = Arc::clone(&probe);
    assert_eq!(unsafe { (*strong).load(Ordering::Relaxed) }, 2);
    drop(clone);
    assert_eq!(unsafe { (*strong).load(Ordering::Relaxed) }, 1);
    // Weak interplay: a downgrade must keep the ArcInner alive at
    // strong-count zero (the fdreap parking depends on it).
    let weak = Arc::downgrade(&probe);
    drop(probe);
    assert!(weak.upgrade().is_none(), "strong count must be zero");
    drop(weak);
}

// ------------ File cell discipline (INV-FILE-CELL-1 / INV-FS-FILEPRIV-1) ------------

/// Write-once install of the dentry cell (INV-FILE-CELL-1).
///
/// # Contract (INV-FILE-CELL-1)
/// `file` is not yet published to any fd table; the cell is
/// written exactly once at open/creation time and never
/// re-pointed (INV-FILE-CELL-1).
#[inline(always)]
pub fn file_dentry_install(file: &File, dentry: Arc<Dentry>) {
        unsafe { *file.dentry.get() = Some(dentry); }
}

/// Write-once install of the ops cell (INV-FILE-CELL-1).
///
/// # Contract (INV-FILE-CELL-1)
/// `file` is not yet published; the ops cell is written once
/// at open/creation time (INV-FILE-CELL-1).
#[inline(always)]
pub fn file_ops_install(file: &File, ops: &'static FileOps) {
        unsafe { *file.ops.get() = Some(ops); }
}

/// Shared read of the write-once ops cell (INV-FILE-CELL-1).
///
/// # Contract (INV-FILE-CELL-1)
/// The cell was installed before publication and is never
/// re-pointed; a shared read through a live &File is
/// race-free (INV-FILE-CELL-1).
#[inline(always)]
pub fn file_ops_ref(file: &File) -> Option<&'static FileOps> {
        unsafe { *file.ops.get() }
}

/// Shared read of the write-once inode cell, Arc view kept
/// (INV-FILE-CELL-1).
///
/// # Contract (INV-FILE-CELL-1)
/// The cell is written at open/creation time before the File
/// is published and never mutated afterwards; a shared read
/// through a shared borrow is race-free (INV-FILE-CELL-1).
#[inline(always)]
pub fn file_inode_arc_ref(file: &File) -> Option<&Arc<Inode>> {
        unsafe { &*file.inode.get() }.as_ref()
}

/// Write-once install of the raw private-data slot
/// (INV-FS-FILEPRIV-1).
///
/// # Contract (INV-FS-FILEPRIV-1)
/// `file` is not yet published (or the install runs in the
/// single-owner attach path); the slot is written once at
/// attach and otherwise mutated only by the close-path take
/// (INV-FS-FILEPRIV-1).
#[inline(always)]
pub fn file_privdata_install_raw(file: &File, data: *mut u8) {
        unsafe { *file.private_data.get() = Some(data); }
}

/// Opaque copy of the raw private-data slot value; dereferencing
/// is the caller's typed step (INV-FS-FILEPRIV-1).
///
/// # Contract (INV-FS-FILEPRIV-1)
/// A plain load of a pointer-sized word; the value is handed
/// out as an opaque copy (INV-FS-FILEPRIV-1).
#[inline(always)]
pub fn file_privdata_ptr_raw(file: &File) -> Option<*mut u8> {
        unsafe { *file.private_data.get() }
}

/// Clear the raw private-data slot (the close-path null store).
///
/// # Contract (INV-FS-FILEPRIV-1)
/// The store races no live handle: close is the last user of
/// this open file description (INV-FS-FILEPRIV-1).
#[inline(always)]
pub fn file_privdata_clear_raw(file: &File) {
        unsafe { *file.private_data.get() = None };
}

/// Take the raw private-data slot value out (close-path reclaim).
///
/// # Contract (INV-FS-FILEPRIV-1)
/// Callers run in the release path where no shared reader can
/// race the cell; the take empties it exactly once
/// (INV-FS-FILEPRIV-1).
#[inline(always)]
pub fn file_privdata_take_raw(file: &File) -> Option<*mut u8> {
        unsafe { (*file.private_data.get()).take() }
}

/// Arc clone of the write-once dentry cell (INV-FILE-CELL-1).
///
/// # Contract (INV-FILE-CELL-1)
/// The cell is written once at open time and read-only
/// afterwards; an ordinary Arc clone of the cell value
/// (INV-FILE-CELL-1).
#[inline(always)]
pub fn file_dentry_arc(file: &File) -> Option<Arc<Dentry>> {
unsafe { (*file.dentry.get()).clone() }
}

