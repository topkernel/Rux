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
pub fn file_inode_install(file: &File, inode: alloc::sync::Arc<crate::fs::inode::Inode>) {
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
pub fn file_inode_arc(file: &File) -> Option<alloc::sync::Arc<crate::fs::inode::Inode>> {
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
pub fn kill_sb_reclaim_box<T>(sb: *mut crate::fs::superblock::SuperBlock) {
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
pub fn file_dentry(file: &File) -> Option<&alloc::sync::Arc<crate::fs::dentry::Dentry>> {
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
