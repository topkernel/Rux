//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The fs subsystem's type definitions (seed-core-design §2c.3).
//!
//! Definitions only. Every `struct`, `enum` and type alias of the
//! subsystem lives here, together with the constants those definitions
//! name (array lengths, and the constants those derive from); every
//! `impl` block stays in the topic file beside the functions that use it —
//! an impl is code, a definition is data layout. The exceptions are
//! definitions local to code: a type declared inside a function or
//! inside an inline module. Traits, statics and every other constant
//! stay with the code that uses them.
//!
//! This file holds no `unsafe`. A field that was private to its old
//! topic file is `pub(super)` here because the code that touches it
//! stayed in that file: the fs directory is the smallest visibility
//! boundary Rust offers for a definition that lives in a sibling module.
//!
//! One section per topic file, in the order the old module declarations
//! had; inside a section the definitions keep their original order.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::{Cell, UnsafeCell};
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, AtomicU32, AtomicU64, AtomicUsize};

use crate::drivers::blkdev;
use crate::errno::Errno;
use crate::process::task::Task;
use crate::process::wait::WaitQueueHead;
use crate::sync::rwlock::RwSpinlock;
use crate::sync::spinlock::Spinlock;

// ---------------------------------------------------------------------------
// file: the open file, its flags, the ops table and its builder, the
// descriptor table with its entry and slot, the fast-fd table and MAX_FDS
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct FileFlags(pub(super) u32);

#[repr(C)]
pub struct FileOps {
    /// Read file
    pub read: Option<fn(&File, &mut [u8]) -> isize>,
    /// Write file
    pub write: Option<fn(&File, &[u8]) -> isize>,
    /// Seek file position
    pub lseek: Option<fn(&File, isize, i32) -> isize>,
    /// Close file
    pub close: Option<fn(&File) -> i32>,
    /// Poll for readiness (takes requested events mask, returns ready events mask)
    pub poll: Option<fn(&File, u16) -> u16>,
    /// file_operations.flush: invoked when an fd is closed (and at
    /// fork/inherit checks) — the file's chance to interrupt waiters
    /// before the description goes away (reference binder_flush wakes
    /// every waiting looper with looper_need_return; plain files have
    /// no flush). Distinct from `close` (last-reference release).
    pub flush: Option<fn(&File)>,
    /// Positional read at an explicit offset, never touching the shared
    /// file position (R71 demand-fault race: the generic read_at fallback
    /// swings the file position through set_pos/read/restore — two
    /// threads sharing this open file description (CLONE_VM peers: one
    /// demand-faulting an mmap page, one in plain read()) interleaved the
    /// sequence and the faulting page got filled from the OTHER thread's
    /// offset; a libc .text page served .rodata bytes and multithreaded
    /// loaders crashed on their own libraries). File ops used for mmap or
    /// pread at scale (ext4, tmpfs) provide a native entry; the generic
    /// fallback in File::read_at covers the rest.
    pub read_at: Option<fn(&File, u64, &mut [u8]) -> isize>,
    /// Positional write at an explicit offset, never touching the shared
    /// file position — the write twin of `read_at` (the R71 race's write
    /// side: ext4's set_pos/write/restore fallback raced concurrent plain
    /// writes on the same description). Files without the slot keep the
    /// generic position-dance fallback below.
    pub write_at: Option<fn(&File, u64, &[u8]) -> isize>,
    /// Demand-populate the page cache for one page-shaped file offset and
    /// report success (the arch page-fault fill op for file mappings).
    /// Presence is ALSO the mm policy's "page-cache-backed demand-paged
    /// file" probe — a data-field check, no fn-pointer call, no filesystem
    /// named (the rung-5 fault seam).
    pub fault: Option<fn(&File, u64) -> bool>,
}

pub struct FileOpsBuilder {
    pub(super) ops: FileOps,
}

/// The operation kinds the File dispatchers route. The errno a caller
/// observes when a file type lacks the capability is decided in ONE
/// place — `File::missing_op_errno` below — with the ABI reasoning
/// attached; dispatch sites stop re-deriving per-callsite conventions.
/// (The §1 paid incident: read() on a valid epoll fd once returned EBADF
/// because the read slot was NULL — an OH event loop treated it as
/// "fd closed" and spun 155,991 times instead of taking its EINVAL
/// error path.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOpKind {
    Read,
    Write,
    LSeek,
    /// Explicit-position read (pread(2)). The errno depends on WHY the
    /// object is not positional (stream type vs op-less type), so the
    /// decision consults the ops table, not just the one slot.
    ReadAt,
    /// Poll has no errno on the syscall path — the missing-poll
    /// fallback is the always-ready mask (`File::poll_dispatch`). The
    /// kind exists so the capability matrix is nameable per op.
    Poll,
}

/// F4 route-4: one getdents64 listing cached per open file description.
/// `file_getdents64` used to re-materialize the WHOLE directory on every
/// call (ext4 list_dir: read every block + clone every entry; 27.6K full
/// re-scans per boot, getdents avg 407 us — the f4 route map's route 4).
/// A cacheable filesystem (INodeOps::readdir_cacheable) stores its
/// listing here keyed by (inode identity, VFS_DIR_EPOCH); any successful
/// directory mutation bumps the epoch, so the next call re-materializes.
/// The entries include the synthesized "." and ".." (the synthesis runs
/// once at materialization instead of per call). `epoch` is the epoch
/// observed BEFORE materialization; the store additionally verifies the
/// epoch did not move DURING materialization (a mutation racing the scan
/// means the listing is a torn mix — serve it for this call as the old
/// path did, but do not pin it in the cache).
pub struct DirentCacheSnapshot {
    /// Identity of the listed inode (Arc target address; the File's
    /// inode slot is written once at open — if it ever changes, this
    /// mismatch forces a re-materialization).
    pub inode_id: usize,
    /// VFS_DIR_EPOCH when materialization started.
    pub epoch: u64,
    /// Full ordered listing WITH the dot entries synthesized.
    pub entries: alloc::vec::Vec<VfsDirEntry>,
}

#[repr(C, align(16))]
pub struct File {
    /// R31-B3: set by close_fd when the slot was removed but an in-flight
    /// syscall still holds a clone — the final dropper runs the close op.
    pub close_pending: core::sync::atomic::AtomicBool,
    /// File flags stored as AtomicU32 for lock-free concurrent access
    /// (fcntl F_SETFL vs read/write data race fix).
    pub flags: AtomicU32,
    /// File position
    pub pos: Spinlock<u64>,
    /// Associated inode
    pub inode: UnsafeCell<Option<Arc<Inode>>>,
    /// Associated dentry
    pub dentry: UnsafeCell<Option<Arc<Dentry>>>,
    /// File operation functions
    pub ops: UnsafeCell<Option<&'static FileOps>>,
    /// Private data (for device-specific data)
    pub private_data: UnsafeCell<Option<*mut u8>>,
    /// close-on-exec flag (FD_CLOEXEC)
    pub cloexec: Spinlock<bool>,
    /// R36-B1: unique open-file-description id (see
    /// FILE_ID_GENERATION_CPU). Written once in File::new before the
    /// value is shared; readers only need a plain u64 load (the Arc
    /// publication orders it).
    pub file_id: u64,
    /// Serializes the whole read/write position sequence (get_pos →
    /// data op → set_pos) so concurrent readers/writers sharing this open
    /// file description cannot interleave and lose offset updates
    /// (LTP fork07: 100 forked children each read 1 byte; two CPUs both
    /// observing pos=K and both storing K+1 left one byte unconsumed and
    /// the parent's final read() returned 1 instead of 0). Sleeping
    /// mutex, not a spinlock: the critical section spans inode read/write
    /// which may sleep in block I/O (a Spinlock guard would pin
    /// preempt_count across that window — the exact hazard b31ccab fixed
    /// for VFS_MUTATION_LOCK). This is Linux's f_pos_lock role.
    pub io_lock: crate::sync::semaphore::Mutex,
    /// O_ASYNC signal destination (F_SETOWN / F_SETOWN_EX). kind 0 none,
    /// 1 process, 2 process group, 3 thread (F_OWNER_TID).
    pub f_owner: Spinlock<FileOwner>,
    /// O_ASYNC signal signal number (F_SETSIG); 0 = default SIGIO.
    pub f_signum: core::sync::atomic::AtomicI32,
    /// F4 route-4: cached getdents64 listing (see DirentCacheSnapshot).
    /// Arc so the hit path is one refcount bump — cloning the Vec per
    /// call would reintroduce the per-call allocation the cache removes.
    /// Materialization (readdir) may sleep in block I/O and runs OUTSIDE
    /// this lock; only the slot swap is under it.
    pub dirent_cache: Spinlock<Option<alloc::sync::Arc<DirentCacheSnapshot>>>,
    /// Embedded readahead state (the reference's struct file carries
    /// file_ra_state inline — oc-rc: the Box-in-private_data form paid a
    /// slab alloc + free per open-then-read cycle, which every
    /// read-cached workload performs per sample). Lazily initialized in
    /// place at the first read by the filesystem that owns the
    /// description (ext4 today); mutated only under io_lock (the same
    /// serialization the Box form had).
    pub ra: UnsafeCell<Option<ReadAheadState>>,
}

/// fasync owner set via F_SETOWN/F_SETOWN_EX (kept on the open file
/// description, like Linux's f_owner).
#[derive(Clone, Copy)]
pub struct FileOwner {
    pub kind: u8,
    pub id: i32,
}

/// Maximum number of file descriptors per table (P1 rlimits, 2026-09).
/// Raised 1024 → 4096 to match the default RLIMIT_NOFILE hard ceiling;
/// per-process enforcement against task.rlimits happens in
/// alloc_fd_from, so unprivileged tasks still get the 1024 soft default
/// and only see more after raising it.
pub const MAX_FDS: usize = 1024;

/// FdTable entry stored on the heap
pub(super) struct FdTableEntry {
    /// Per-descriptor close-on-exec bits. FD_CLOEXEC belongs to the
    /// DESCRIPTOR in POSIX, not the underlying description: a file opened
    /// O_CLOEXEC at fd N and dup2'd to fd M must leave M WITHOUT the flag.
    /// The old per-File flag leaked O_CLOEXEC through dup2 — musl-based
    /// shells open redirections O_CLOEXEC, dup2 to stdio, exec, and lost
    /// their stdio fds (found via musl __init_libc's deliberate NULL-crash
    /// on POLLNVAL + missing /dev/null).
    ///
    /// (Leg-52 lever B: the `fds` array moved OUT of the entry into
    /// FdTable::slots — a lock-free AtomicPtr array the read path never
    /// takes the entry lock for. This struct now holds only metadata.)
    pub(super) cloexec_bits: [u64; MAX_FDS / 64],
    /// Occupancy bitmap (perf-gaps G2, 2026-11): one bit per slot,
    /// maintained inside the SAME entry-lock sections that publish or
    /// clear the slot itself (install_fd / close_fd / dup2_fd / fork_from)
    /// — never read outside the entry lock, where it is exact by
    /// construction. Full-table walkers (`for_each_file`, the close-time
    /// epoll purge) iterate these 16 words instead of polling 1024
    /// lock-free slots: epoll_purge_closed_fd's scan was measured at
    /// ~8.9 us per close under KVM — nearly the whole p6_openclose gap —
    /// and it runs on the last-reference close of EVERY file.
    pub(super) open_bits: [u64; MAX_FDS / 64],
    pub(super) next_fd: usize,
    pub(super) count: usize,
}

/// One lock-free fd slot: the Arc<File> as a raw pointer, null = empty.
/// Single-word aligned stores, so a lock-free load can never observe a
/// torn value.
pub(super) struct FdSlot(pub(super) core::sync::atomic::AtomicPtr<File>);

pub struct FdTable {
    /// Lock-free slot array (leg-52 lever B, perf-report 8.10.22):
    /// get_file ran 132-144M times/boot through entry.lock_irqsave —
    /// an irq_save + preempt_count pair + a contended per-TABLE TTAS RMW
    /// per lookup. Readers are now RCU readers (see get_file): an
    /// Acquire slot load + strong-count increment inside a read-side
    /// critical section, no lock and no bank claim. Writers mutate
    /// slots with single-word stores under the entry lock and park any
    /// removed occupant in the global retire queue (fs::fdreap), which
    /// drops it after a full RCU grace period.
    pub(super) slots: alloc::boxed::Box<[FdSlot; MAX_FDS]>,
    /// Writer serialization + descriptor metadata (cloexec bits,
    /// allocation hint, count).
    pub(super) entry: crate::sync::spinlock::Spinlock<Box<FdTableEntry>>,
}

/// fdget's two outcomes (the reference's struct fd): a no-refcount
/// borrow on an unshared table (FDPUT clear) or a full Arc reference
/// (fget) when CLONE_FILES sharers exist.
pub enum FastFd {
    Borrowed(&'static File),
    Owned(Arc<File>),
}

// ---------------------------------------------------------------------------
// event_notify: the epoll event record, the wake function pointer, the epoll
// wake node and shard, the signalfd wake registry
// ---------------------------------------------------------------------------

/// epoll_event structure
// Linux uapi packs epoll_event ONLY on x86-64
// (include/uapi/linux/eventpoll.h: EPOLL_PACKED under __x86_64__); every
// other arch — riscv64 included — uses the natural C layout: events@0,
// data@8, 16-byte records. glibc and musl mirror the uapi (verified:
// Ubuntu 22.04 riscv64 glibc sizeof(epoll_event)=16, data@8; musl 1.2.5
// applies __packed__ only under __x86_64__).
//
// R72: making the struct unconditionally packed fixed the x86_64 stride
// (LTP epoll_wait01) but broke riscv64: the kernel wrote 12-byte records
// with data@4 while both riscv64 libcs read 16-byte records with data@8,
// so every `data` cookie came back as a mix of padding and neighboring
// bytes — Xorg's InputThread dereferenced a mangled epoll cookie and
// SIGSEGV'd at 0x4000b80 on every boot (deterministic, t=8s), which with
// force-unblocked synchronous signals took the whole server down. Keep
// each arch byte-identical to its uapi shape.
#[cfg_attr(feature = "x86_64", repr(C, packed))]
#[cfg_attr(not(feature = "x86_64"), repr(C))]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct EPollEvent {
    pub events: u32,
    pub data: u64,
}

/// One (file_id → epoll instance) binding. `list` is the FIRST field so a
/// node pointer doubles as its `ListHead` pointer (the container_of cast
/// is then a no-op) — same trick as `RcuHead`.
/// Per-node wake trampoline: performs the pinned re-arm + wake for the
/// target the registry stored (type-erased at registration — the
/// register side supplies a monomorphized fn wrapping its instance
/// type; no service type crosses into the framework).
pub type WakeFn = fn(ep: *mut u8, file_id: u64);

pub struct EpollWakeNode {
    pub list: crate::list::ListHead,
    pub file_id: u64,
    pub ep: *mut u8,
    pub wake: WakeFn,
}

/// ALLOCATION-FREE SHARD BODY (115th, allocator-under-shard-lock hunt):
/// the shard is an intrusive list of `EpollWakeNode`s — linking and
/// unlinking are two pointer writes, and the NODE is Box-allocated /
/// freed OUTSIDE the shard's irqsave section. The previous
/// `Vec<(u64, *mut T)>` body pushed (and on capacity growth,
/// ALLOCATED) inside the lock: an allocation whose per-CPU magazine row
/// was empty takes the small_alloc class lock and can walk pages / touch
/// the buddy under it — sections measured at 300 ms of real KVM time
/// under OH-boot churn (rux:allocwatch (retired debug=allocwatch), lb268). For that whole window
/// every producer mapping to the shard spins IRQs-off in
/// epoll_notify_file's lock_irqsave, no CPU on the box reports a
/// quiescent state, and the fdreap grace round prints
/// RCU-GP-BOUND site=fdreap-close — the exact print that fires
/// immediately before the OH boot deadlock wedge (DEADLOCK: spinlock
/// stuck ... EPOLL_WAKE_SHARDED + soft lockups). The sentinel cannot
/// self-reference in a const initializer, so it is lazily `init()`ed on
/// first acquisition (`init_once`, under the shard lock).
pub struct EpollWakeShard {
    pub head: crate::list::ListHead,
}

/// Owning-task → signalfd wait queue registry for signal-arrival wakeups.
/// Task pointers are compared for identity only (never dereferenced), so a
/// recycled task address causes at worst a spurious wake (readers re-check
/// readiness); entries are removed in the fd's close op.
///
/// Wrapper so the raw-pointer registry is `Send` (access under the lock).
pub struct SignalfdWakeRegistry {
    pub bindings: alloc::vec::Vec<
        (*mut crate::process::task::Task, *const crate::process::wait::WaitQueueHead),
    >,
}

// ---------------------------------------------------------------------------
// inode: the inode number, the directory-entry record, the mode, the inode
// ops table, the state, the anonymous-device map, the inode, and the
// inode cache with its hash buckets and stats
// ---------------------------------------------------------------------------

/// Inode number type
pub type Ino = u64;

/// Unified directory entry returned by inode.ops.readdir.
/// All filesystems convert their internal entry types to this.
pub struct VfsDirEntry {
    /// Inode number
    pub ino: u64,
    /// Entry name (UTF-8, no NUL terminator)
    pub name: alloc::vec::Vec<u8>,
    /// File type (DT_REG, DT_DIR, DT_CHR, DT_LNK, etc.)
    pub file_type: u8,
}

/// Inode mode (file type and permissions)
///
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct InodeMode(pub(super) u32);

/// Inode operation function pointer table
///
/// All operations take:
/// - `dir`: Parent directory inode (for create/unlink/mkdir/rmdir)
/// - `name`: Entry name
/// - Additional parameters as needed
///
/// Returns:
/// - 0 on success
/// - Negative errno on failure
#[repr(C)]
pub struct INodeOps {
    // ==================== Directory Operations ====================

    /// Lookup entry in directory
    /// Returns inode number if found, or error
    pub lookup: Option<fn(&Inode, &[u8]) -> Result<Ino, Errno>>,

    /// Create regular file
    pub create: Option<fn(&Inode, &[u8], InodeMode) -> Result<Arc<Inode>, Errno>>,

    /// Create hard link
    /// Arguments: (dir, name, target_inode)
    pub link: Option<fn(&Inode, &[u8], &Inode) -> i32>,

    /// Remove directory entry (unlink file)
    pub unlink: Option<fn(&Inode, &[u8]) -> i32>,

    /// Create symbolic link
    /// Arguments: (dir, name, target_path)
    pub symlink: Option<fn(&Inode, &[u8], &[u8]) -> Result<Arc<Inode>, Errno>>,

    /// Create directory
    pub mkdir: Option<fn(&Inode, &[u8], InodeMode) -> Result<Arc<Inode>, Errno>>,

    /// Remove empty directory
    pub rmdir: Option<fn(&Inode, &[u8]) -> i32>,

    /// Create device node (mknod)
    pub mknod: Option<fn(&Inode, &[u8], InodeMode, u64) -> Result<Arc<Inode>, Errno>>,

    /// Rename file/directory
    /// Arguments: (old_dir, old_name, new_dir, new_name)
    pub rename: Option<fn(&Inode, &[u8], &Inode, &[u8]) -> i32>,

    // ==================== Symlink Operations ====================

    /// Read symbolic link target
    /// Returns number of bytes written to buffer, or negative errno
    pub readlink: Option<fn(&Inode, &mut [u8]) -> isize>,

    // ==================== File Operations (delegated) ====================

    /// Get file operations for this inode
    /// This allows inode-specific file operations
    pub get_file_ops: Option<fn(&Inode) -> Option<&'static FileOps>>,

    /// List directory entries.
    /// Returns a vector of VfsDirEntry on success, None if not a directory / error.
    pub readdir: Option<fn(&Inode) -> Option<alloc::vec::Vec<VfsDirEntry>>>,

    /// Incremental directory read — the reference kernel's ext4_readdir
    /// shape (getdents lane, 2026-11-17). Fills `buf` with packed
    /// linux_dirent64 records (d_ino u64 / d_off u64 / d_reclen u16 /
    /// d_type u8 / name / NUL / zero pad to 8) starting at directory
    /// BYTE offset `pos`, and returns `(bytes_filled, next_pos)`.
    ///
    /// Contract:
    /// - `next_pos` is the byte offset of the first entry NOT emitted
    ///   (the resume point; getdents64 stores it as the file position,
    ///   and each record's d_off names it — telldir/seekdir round-trip
    ///   through lseek with no conversion).
    /// - Zero bytes filled means EOF (pos at/after the directory size,
    ///   or nothing emittable remains). Err(EINVAL) when the buffer is
    ///   too small for even one entry but emittable entries remain.
    /// - The op writes EVERY byte of [0..bytes_filled) — pad bytes
    ///   included — so the syscall layer may copy exactly that prefix
    ///   to userspace without a pre-zeroing pass.
    /// - The op may sleep (block I/O); callers hold no spinlocks.
    ///
    /// A filesystem providing this op serves getdents64 WITHOUT
    /// whole-directory materialization (no Vec<VfsDirEntry>, no
    /// per-name heap allocation) and bypasses the per-open DirentCache
    /// snapshot: each call reads only the directory blocks that cover
    /// [pos, pos + buf.len()), exactly like the reference's
    /// block-at-a-time filldir loop. `readdir` above stays as the
    /// Vec-returning fallback for callers that want whole listings.
    pub readdir_fill:
        Option<fn(&Inode, pos: u64, buf: &mut [u8]) -> Result<(usize, u64), i32>>,

    /// F4 route-4 (getdents64 cache): whether this filesystem's
    /// directory listings may be cached across getdents64 calls on one
    /// open file description. True is ONLY safe for filesystems whose
    /// directory content changes exclusively through the INodeOps
    /// mutation entry points above (create/link/unlink/symlink/mkdir/
    /// rmdir/mknod/rename) — every successful dispatch bumps the global
    /// VFS directory epoch (vfs::VFS_DIR_EPOCH), and the cache re-checks
    /// that epoch, so any mutation invalidates every cached listing.
    /// Dynamic filesystems whose content changes WITHOUT a mutation op
    /// (procfs: process spawn/exit; devfs: device registration; sysfs/
    /// debugfs/cgroupfs/selinuxfs: live state) MUST set false.
    pub readdir_cacheable: bool,

    /// Called when a file is opened. Allows FS to set up file.private_data.
    /// Returns 0 on success, negative errno on failure.
    pub open: Option<fn(&Inode, &File) -> i32>,

    // ==================== Permission Operations ====================

    /// Check permission
    /// Returns 0 if allowed, negative errno if denied
    pub permission: Option<fn(&Inode, u32) -> i32>,

    // ==================== Attribute Operations ====================

    /// Get attributes (stat)
    /// Returns 0 on success, fills in stat structure
    pub getattr: Option<fn(&Inode, &mut Stat) -> i32>,

    /// Set attributes
    /// attr: ATTR_MODE (1), ATTR_UID (2), ATTR_GID (3), ATTR_SIZE (4), ATTR_UID_GID (5)
    /// arg1/arg2: attribute-specific values
    pub setattr: Option<fn(&Inode, u32, u64, u64) -> i32>,

    // ==================== Inode Instantiation ====================

    /// Instantiate a VFS Inode from (parent_inode, name, child_ino).
    /// Called by dentry-based path_lookup after op_lookup returns the inode number.
    /// Each filesystem implements this to create a fully populated VFS Inode.
    pub iget: Option<fn(&Inode, &[u8], Ino) -> Result<Arc<Inode>, Errno>>,

    // ==================== Inode Lifecycle ====================

    /// Called when an inode is about to be freed (refcount drops to zero).
    /// Use this to reclaim private_data (e.g., Arc::from_raw for leaked Arcs).
    pub destroy_inode: Option<fn(&mut Inode)>,
}

/// Inode state
///
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum InodeState {
    /// Newly allocated inode
    INew = 0,
    /// Inode exists
    IExisting = 1,
    /// Inode being deleted
    IDying = 2,
}

pub(super) struct AnonDevMap(pub(super) alloc::collections::BTreeMap<u64, DevNo>);

#[repr(C)]
pub struct Inode {
    // ==================== Core Fields ====================

    /// Filesystem identifier (used as part of icache key to avoid cross-FS collisions)
    pub fs_id: u64,
    /// Inode number (unique within filesystem)
    pub ino: Ino,
    /// Inode mode (file type and permissions)
    pub mode: InodeMode,
    /// File size in bytes
    pub size: AtomicU64,
    /// Device number (for device inodes)
    pub rdev: u64,
    /// Device number (atomic mirror — settable through an Arc<Inode> for
    /// retyped device nodes created by mknod on non-devfs filesystems)
    pub rdev_atomic: AtomicU64,
    /// Owner user ID
    pub uid: AtomicU32,
    /// Owner group ID
    pub gid: AtomicU32,
    /// (negative-lookup memo, 21st successor) Directory content
    /// generation: bumped on every successful mutation dispatched with
    /// this inode as the parent directory (create/mkdir/unlink/rmdir/
    /// rename/link/symlink/mknod). Stamp lives on the INODE, not the
    /// dentry, because bind-mount aliases of one directory are separate
    /// dentries sharing this inode — a memo cached under the sandbox
    /// view must invalidate when the file appears through the host view
    /// (and vice versa). The path walk's memo-hit check compares a
    /// memo child's stamp against this counter.
    pub dir_mut_gen: AtomicU64,
    /// Inode state
    pub state: Spinlock<InodeState>,

    // ==================== Operations ====================

    /// Inode operations (filesystem-specific)
    pub ops: Option<&'static INodeOps>,

    // ==================== Filesystem Linkage ====================

    /// Pointer to superblock (filesystem this inode belongs to)
    /// This is a raw pointer to avoid circular references
    pub sb: Option<*mut u8>,  // Points to filesystem-private data (e.g., Ext4Inode)

    // ==================== Private Data ====================

    /// Private data for filesystem-specific use
    /// For RootFS: points to RootFSNode
    /// For ext4: points to Ext4Inode
    pub private_data: Option<*mut u8>,

    // ==================== Data ====================

    /// File data (used for memory-backed files like RootFS)
    /// For block-backed filesystems, this is None and data is read from disk
    pub data: Spinlock<Option<FileBuffer>>,

    /// Extended attributes (P1 xattr minimal): name → value, allocated
    /// lazily on the first set. In-memory only — ext4 does not persist
    /// them and an icache eviction of the inode loses the set (documented
    /// limitation; a real on-disk xattr block is future work).
    pub xattrs: Spinlock<Option<alloc::collections::BTreeMap<alloc::vec::Vec<u8>, alloc::vec::Vec<u8>>>>,

    /// chattr-style file flags (FS_*_FL — FS_IMMUTABLE_FL 0x10,
    /// FS_APPEND_FL 0x20, ...), as served by FS_IOC_GETFLAGS/SETFLAGS.
    /// ext4 seeds this from the on-disk i_flags at iget and SETFLAGS
    /// persists it back; all other filesystems keep it in-memory only
    /// (same lifetime caveat as `xattrs`).
    pub ioc_flags: AtomicU32,

    // ==================== Open-description counts ====================
    // (stat-2x lane) The i_readcount/i_writecount analogue, per inode.
    // These replaced a GLOBAL Spinlock<BTreeMap<(u64,u64), OpenCounts>>
    // that every open() and close() locked and walked (register: lock +
    // tree entry; unregister: lock + get_mut + maybe remove) — the
    // readers (execve's ETXTBSY gate, the F_SETLEASE grant checks,
    // break_lease's pending-breaker counts) all arrive with the inode
    // in hand, so per-inode atomics answer them directly. Reference
    // parity: Linux keeps i_readcount/i_writecount as per-inode atomic_t
    // for exactly this reason.
    /// Live open file descriptions on this inode (total).
    pub open_total: AtomicU32,
    /// Of those, the ones opened for write access (O_WRONLY/O_RDWR).
    pub open_write: AtomicU32,
    /// Openers currently BLOCKED in break_lease (pending total).
    pub open_breaking: AtomicU32,
    /// Of those, blocked WRITE-mode breakers (see locks::break_lease).
    pub open_breaking_write: AtomicU32,

    // ==================== Reference Counting ====================

    /// Reference count
    pub(super) ref_count: AtomicU64,
}

/// Inode cache statistics
#[derive(Debug)]
pub struct InodeCacheStats {
    /// Cache hit count
    pub hits: AtomicU64,
    /// Cache miss count
    pub misses: AtomicU64,
    /// Eviction count
    pub evictions: AtomicU64,
}

/// Hash table bucket
pub(super) struct InodeHashBucket {
    /// inode pointer
    pub(super) inode: Option<Arc<Inode>>,
    /// inode number (for quick comparison)
    pub(super) ino: Ino,
    /// filesystem identifier (for cross-FS uniqueness)
    pub(super) fs_id: u64,
    /// LRU timestamp (for eviction)
    pub(super) access_time: AtomicU64,
}

/// Inode hash table
pub(super) struct InodeCache {
    /// Hash table (heap-allocated to avoid large stack allocation)
    pub(super) buckets: alloc::boxed::Box<[InodeHashBucket]>,
    /// Number of entries in cache
    pub(super) count: usize,
    /// Global timestamp (for LRU)
    pub(super) global_time: AtomicU64,
    /// Statistics
    pub(super) stats: InodeCacheStats,
}

// ---------------------------------------------------------------------------
// dentry: the dentry flags and state, the internal mount, the reader bank,
// the walk claim, the child-memo tables and the constants they are sized by,
// the dentry, and the dentry cache with its hash bucket and stats
// ---------------------------------------------------------------------------

/// Dentry state flags
///
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct DentryFlags(pub(super) u32);

/// Dentry state
///
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum DentryState {
    /// Dentry unused
    DUnhashed,
    /// Dentry used
    DHashed,
    /// Dentry being deleted
    DKill,
}

/// VFS mount descriptor — attached to a dentry that is a mount point.
///
/// When `follow_mount()` encounters a dentry with a `VfsMountInternal`,
/// it transparently switches to `root` (the mounted filesystem's root dentry).
pub struct VfsMountInternal {
    /// Root dentry of the mounted filesystem
    pub root: Arc<Dentry>,
    /// Mount flags (interior-mutable: mount(2) with MS_REMOUNT updates them
    /// in place while concurrent lookups read them on other CPUs)
    pub flags: Spinlock<MntFlags>,
    /// sc181: global attach sequence (monotonic). The MS_REC clone scan
    /// (fs::vfs::collect_subtree_mounts) uses it to emit the clone set in
    /// attach order — identical to the old registry-order linear scan —
    /// so stacked clones always attach parents before children.
    pub seq: u64,
}

/// One per-CPU reader-nesting counter, padded to its own cacheline so
/// the CPUs' counter RMWs never share a line (FdTable discipline).
#[repr(align(64))]
pub(super) struct DentryReaderBank(pub(super) AtomicUsize);

/// Walk-scoped reader claim. `acquire()` bumps this CPU's bank (SeqCst —
/// the claim is globally visible before any subsequent pointer load
/// executes); Drop decrements it (Release). The value is Copy-able
/// evidence of holding the claim: methods taking `&WalkClaim` may load
/// raw pointers that are only guaranteed alive while some claim is held
/// on the calling CPU.
pub struct WalkClaim {
    /// Bank index claimed, or usize::MAX for the slow-path lock (whose
    /// guard we keep alive in `slow`).
    pub(super) cpu: usize,
    pub(super) slow: Option<crate::sync::spinlock::SpinlockIrqGuard<'static, ()>>,
}

/// Inline name capacity of a memo slot (24 bytes covers the hot
/// component shapes; longer names take the locked path).
pub(super) const CHILD_MEMO_NAME_MAX: usize = 24;
/// Memo slots per dentry (index = hash & mask).
pub(super) const CHILD_MEMO_SLOTS: usize = 4;
pub(super) const CHILD_MEMO_MASK: usize = CHILD_MEMO_SLOTS - 1;

/// (stat-2x lane) The big-memo allocation: SELF-DESCRIBING so a
/// lock-free reader that Acquire-loads the published base can size its
/// slice without a second mirror that could race a concurrent grow
/// (old base + new count would read past the old table). `nslots` is
/// immutable after construction — the base pointer alone identifies a
/// consistent (slots, len) pair.
pub(super) struct BigMemoTable {
    pub(super) nslots: usize,
    pub(super) slots: alloc::vec::Vec<ChildMemoSlot>,
}

/// Plain (non-atomic) slot payload — read/written through the enclosing
/// slot's seqlock (the sync::seqlock UnsafeCell discipline).
pub(super) struct ChildMemoData {
    /// `children_ver` at fill time — the slot is valid only while it
    /// equals the dentry's current version (any map mutation bumped it).
    pub(super) ver: u64,
    /// FNV-1a of the name (fast reject before the byte compare).
    pub(super) hash: u64,
    /// Name bytes, `name_len` valid.
    pub(super) name: [u8; CHILD_MEMO_NAME_MAX],
    pub(super) name_len: u8,
    /// The child (raw — the child map / a pending retire keeps it alive;
    /// see the reader-bank protocol).
    pub(super) child: *const Dentry,
}

/// One lock-free child-memo slot, exactly one cacheline. Racing readers
/// are guarded by the per-slot seqlock (odd = a refill is writing).
#[repr(align(64))]
pub(super) struct ChildMemoSlot {
    /// Per-slot seqlock (writer: odd → fields → even).
    pub(super) seq: AtomicU64,
    pub(super) data: core::cell::UnsafeCell<ChildMemoData>,
}

/// Directory entry
///
#[repr(C)]
pub struct Dentry {
    /// dentry name (last component, e.g. "null" not "/dev/null")
    pub name: Spinlock<String>,
    /// parent directory entry
    pub parent: Spinlock<Option<Arc<Dentry>>>,
    /// child dentries (name -> dentry mapping)
    pub children: Spinlock<BTreeMap<String, Arc<Dentry>>>,
    /// associated inode
    pub inode: Spinlock<Option<Arc<Inode>>>,
    /// If this dentry is a mount point, points to the mount descriptor.
    /// None means this dentry is not a mount point.
    pub vfsmount: Spinlock<Option<Arc<VfsMountInternal>>>,
    /// Only set on a MOUNTED FILESYSTEM'S ROOT dentry: the mountpoint
    /// dentry in the parent filesystem. Weak on purpose — the mountpoint
    /// dentry holds the mount (`vfsmount`) which holds this dentry
    /// strongly; a strong edge back would form an Arc cycle. Lets
    /// `build_path()` cross the mount boundary: a mounted fs root has no
    /// `parent` pointer, so without this hop paths of entries inside a
    /// mount lose the mountpoint prefix (chdir("/proc") stored cwd="/").
    pub(super) mountpoint: Spinlock<Option<alloc::sync::Weak<Dentry>>>,
    /// dentry state
    pub state: Spinlock<DentryState>,
    /// dentry flags
    pub flags: Spinlock<DentryFlags>,
    /// reference count
    pub(super) ref_count: AtomicU64,
    /// Negative dentry: lookup found no file on disk
    pub negative: AtomicBool,
    /// (perf-path lane) Mirror of `vfsmount.is_some()`: lets the walk's
    /// per-component mount test run as ONE Relaxed load instead of the
    /// vfsmount spinlock + Option<Arc> clone. Maintained by
    /// set_mount/clear_mount, which remain the serialized writers of the
    /// authoritative field.
    pub(super) is_mountpoint: AtomicBool,
    /// (perf-path lane) icache identity generation at the time `inode`
    /// was last refreshed against the icache (see inode::icache_regen).
    /// When it equals the current generation, the cached Arc IS the
    /// icache's current object for (ino, fs_id) and the walk skips the
    /// global-lock re-check. Relaxed is sufficient: the load only decides
    /// whether to take the SLOW path — a stale "current" verdict is
    /// caught on the next walk after the generation bumps.
    pub(super) inode_gen: AtomicU64,
    /// (21st successor, negative-lookup memo) The parent directory
    /// INODE's content generation at insert time (meaningful only while
    /// `negative` is set). The stamp lives on the shared inode —
    /// bind-mount aliases are separate dentries over one inode, so a
    /// mutation through ANY alias invalidates memos cached under every
    /// alias. 0 = unstamped.
    pub neg_gen: AtomicU64,
    /// Negative memo children currently cached under this parent
    /// (bounded — see NEG_MEMO_CAP / memo_negative).
    pub(super) neg_count: AtomicUsize,
    /// (gap-fs lane) Child-map mutation version — bumped (under the
    /// children lock) by every add/remove/clear/rehome. A memo slot
    /// stamped with an older version is stale and ignored, so readers
    /// never need to serialize with the mutators.
    pub(super) children_ver: AtomicU64,
    /// (gap-fs lane) Lock-free child memo — see ChildMemoSlot.
    pub(super) memo: [ChildMemoSlot; CHILD_MEMO_SLOTS],
    /// (stat-2x lane) Secondary memo for large directories — ownership
    /// slot. Swapped (None → Some → replaced on growth) only under the
    /// children lock; lock-free readers go through the `big_memo_ptr`
    /// mirror and the table's self-describing length. Replaced tables
    /// are parked in DENTRY_RET_MEMO; the final table frees with the
    /// Dentry.
    pub(super) big_memo: core::cell::UnsafeCell<Option<alloc::boxed::Box<BigMemoTable>>>,
    /// (stat-2x lane) Published pointer of the secondary memo table
    /// (null = not allocated). Release-stored under the children lock;
    /// readers Acquire-load it under a walk claim and read the table's
    /// immutable `nslots` for the slice length.
    pub(super) big_memo_ptr: AtomicPtr<BigMemoTable>,
    /// (gap-fs lane) Mirror of the `inode` slot's Arc pointer (null =
    /// None), published with Release by set_inode/clear_inode. Readers
    /// load it under a WalkClaim and may increment the strong count
    /// directly (the claim blocks the drain; see the reader-bank
    /// protocol comment).
    pub(super) inode_ptr: AtomicPtr<Inode>,
}

/// Dentry cache statistics
#[derive(Debug)]
pub struct DentryCacheStats {
    /// Cache hit count
    pub hits: AtomicU64,
    /// Cache miss count
    pub misses: AtomicU64,
    /// Eviction count
    pub evictions: AtomicU64,
}

/// Hash table bucket
pub(super) struct DentryHashBucket {
    /// dentry pointer
    pub(super) dentry: Option<Arc<Dentry>>,
    /// hash key (for quick comparison)
    pub(super) key: u64,
    /// parent inode number (for hash collision disambiguation)
    pub(super) parent_ino: u64,
    /// LRU timestamp (for eviction)
    pub(super) access_time: AtomicU64,
}

/// Dentry hash table
pub(super) struct DentryCache {
    /// Hash table (heap-allocated to avoid large stack allocation)
    pub(super) buckets: alloc::boxed::Box<[DentryHashBucket]>,
    /// Number of entries in cache
    pub(super) count: usize,
    /// Global timestamp (for LRU)
    pub(super) global_time: AtomicU64,
    /// Statistics
    pub(super) stats: DentryCacheStats,
}

// ---------------------------------------------------------------------------
// elf: the ELF64 header and program header, the type, machine and
// segment-type enums, the load info, the loader marker and the error
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Ehdr {
    /// Magic number and other info
    pub e_ident: [u8; 16],
    /// File type
    pub e_type: u16,
    /// Machine type
    pub e_machine: u16,
    /// Version
    pub e_version: u32,
    /// Entry point address
    pub e_entry: u64,
    /// Program header table offset
    pub e_phoff: u64,
    /// Section header table offset
    pub e_shoff: u64,
    /// Processor-specific flags
    pub e_flags: u32,
    /// ELF header size
    pub e_ehsize: u16,
    /// Program header table entry size
    pub e_phentsize: u16,
    /// Program header table entry count
    pub e_phnum: u16,
    /// Section header table entry size
    pub e_shentsize: u16,
    /// Section header table entry count
    pub e_shnum: u16,
    /// Section header string table index
    pub e_shstrndx: u16,
}

#[repr(u16)]
#[derive(Debug, Copy, Clone, PartialEq)]
#[allow(non_camel_case_types)]
pub enum ElfType {
    /// Unknown type
    ET_NONE = 0,
    /// Relocatable file
    ET_REL = 1,
    /// Executable file
    ET_EXEC = 2,
    /// Shared object file
    ET_DYN = 3,
    /// Core file
    ET_CORE = 4,
}

#[repr(u16)]
#[derive(Debug, Copy, Clone, PartialEq)]
#[allow(non_camel_case_types)]
pub enum ElfMachine {
    /// No machine
    EM_NONE = 0,
    /// AT&T WE 32100
    EM_M32 = 1,
    /// SPARC
    EM_SPARC = 2,
    /// x86
    EM_386 = 3,
    /// Motorola 68000
    EM_68K = 4,
    /// Motorola 88000
    EM_88K = 5,
    /// Intel 80860
    EM_860 = 7,
    /// MIPS
    EM_MIPS = 8,
    /// IBM System/370
    EM_S370 = 9,
    /// MIPS RS3000 Little-endian
    EM_MIPS_RS3_LE = 10,
    /// Hewlett-Packard PA-RISC
    EM_PARISC = 15,
    /// Fujitsu VPP500
    EM_VPP500 = 17,
    /// Enhanced instruction set SPARC
    EM_SPARC32PLUS = 18,
    /// Intel 80960
    EM_960 = 19,
    /// PowerPC
    EM_PPC = 20,
    /// PowerPC 64-bit
    EM_PPC64 = 21,
    /// IBM S390
    EM_S390 = 22,
    /// IBM SPU/SPC
    EM_SPU = 23,
    /// NEC V800
    EM_V800 = 36,
    /// Fujitsu FR20
    EM_FR20 = 37,
    /// TRW RH-32
    EM_RH32 = 38,
    /// Motorola RCE
    EM_RCE = 39,
    /// ARM
    EM_ARM = 40,
    /// DEC Alpha
    EM_ALPHA = 41,
    /// Hitachi SH
    EM_SH = 42,
    /// SPARC-V9
    EM_SPARCV9 = 43,
    /// Siemens Tricore
    EM_TRICORE = 44,
    /// Argonaut RISC Core
    EM_ARC = 45,
    /// Hitachi H8/300
    EM_H8_300 = 46,
    /// Hitachi H8/300H
    EM_H8_300H = 47,
    /// Hitachi H8S
    EM_H8S = 48,
    /// Hemicyle
    EM_H8_500 = 49,
    /// Intel IA-64 processor architecture
    EM_IA_64 = 50,
    /// Stanford MIPS-X
    EM_MIPS_X = 51,
    /// Motorola ColdFire
    EM_COLDFIRE = 52,
    /// Motorola M68HC12
    EM_68HC12 = 53,
    /// Fujitsu MMA Multimedia Accelerator
    EM_MMA = 54,
    /// Siemens PCP
    EM_PCP = 55,
    /// Sony nCPU embedded RISC
    EM_NCPU = 56,
    /// Sony nCPU 20-bit
    EM_NDR1 = 57,
    /// Motorola Star*Core processor
    EM_STARCORE = 58,
    /// Toyota ME16 processor
    EM_ME16 = 59,
    /// STMicroelectronics ST100 processor
    EM_ST100 = 60,
    /// Advanced Logic Corp. TinyJ
    EM_TINYJ = 61,
    /// AMD x86-64 architecture
    EM_X86_64 = 62,
    /// Sony DSP Processor
    EM_PDSP = 63,
    /// Siemens FX66
    EM_FX66 = 66,
    /// STMicroelectronics ST9+ 8/16 mc
    EM_ST9PLUS = 67,
    /// STMicroelectronics ST7 8 bit mc
    EM_ST7 = 68,
    /// Motorola MC68HC16 Microcontroller
    EM_68HC16 = 69,
    /// Motorola MC68HC11 Microcontroller
    EM_68HC11 = 70,
    /// Motorola MC68HC08 Microcontroller
    EM_68HC08 = 71,
    /// Motorola MC68HC05 Microcontroller
    EM_68HC05 = 72,
    /// Silicon Graphics SVx
    EM_SVX = 73,
    /// STMicroelectronics ST19 8 bit mc
    EM_ST19 = 74,
    /// Digital VAX
    EM_VAX = 75,
    /// Axis Communications 32-bit embedded processor
    EM_CRIS = 76,
    /// Infineon Technologies 32-bit embedded processor
    EM_JAVELIN = 77,
    /// FirePath
    EM_FIREPATH = 78,
    /// LSI Logic 16-bit DSP Processor
    EM_ZSP = 79,
    /// MMIX
    EM_MMIX = 80,
    /// Harvard University machine-independent object files
    EM_HUANY = 81,
    /// SiTera Prism
    EM_PRISM = 82,
    /// Atmel AVR 8-bit microcontroller
    EM_AVR = 83,
    /// Fujitsu FR30
    EM_FR30 = 84,
    /// Mitsubishi D10V
    EM_D10V = 85,
    /// Mitsubishi D30V
    EM_D30V = 86,
    /// NEC v850
    EM_V850 = 87,
    /// Mitsubishi M32R
    EM_M32R = 88,
    /// Matsushita MN10300
    EM_MN10300 = 89,
    /// Matsushita MN10200
    EM_MN10200 = 90,
    /// picoJava
    EM_PJ = 91,
    /// OpenRISC 32-bit embedded processor
    EM_OPENRISC = 92,
    /// ARC International ARCompact
    EM_ARC_COMPACT = 93,
    /// Tensilica Xtensa Architecture
    EM_XTENSA = 94,
    /// Alphamosaic VideoCore
    EM_VIDEOCORE = 95,
    /// Thompson Multimedia General Purpose Processor
    EM_TMM_GPP = 96,
    /// National Semiconductor 32000
    EM_NS32K = 97,
    /// Tenor Network Technology TinyCPU
    EM_TPC = 98,
    /// Trebia SIP 32-bit
    EM_SNP1K = 99,
    /// STMicroelectronics ST200
    EM_ST200 = 100,
    /// Ubicom IP2xxx
    EM_IP2K = 101,
    /// MAX Processor
    EM_MAX = 102,
    /// National Semiconductor CompactRISC
    EM_CR = 103,
    /// Fujitsu F2MC16
    EM_F2MC16 = 104,
    /// Texas Instruments embedded microcontroller msp430
    EM_MSP430 = 105,
    /// Analog Devices Blackfin (DSP) processor
    EM_BLACKFIN = 106,
    /// S1C33 Embedded Epson SE
    EM_SE_C33 = 107,
    /// Sharp embedded microprocessor
    EM_SEP = 108,
    /// Arca RISC Microprocessor
    EM_ARCA = 109,
    /// Microprocessor Systems from Fujitsu
    EM_UNICORE = 110,
    /// eXcess: 64-bit CPU
    EM_EXCESS = 111,
    /// IXP12000
    EM_DXP = 112,
    /// Altera Nios II
    EM_ALTERA_NIOS2 = 113,
    /// ThreadX
    EM_CRX = 114,
    /// Standard Performance Corporation
    EM_XGATE = 115,
    /// Intel Timelay
    EM_C166 = 116,
    /// Renesas M16C series
    EM_M16C = 117,
    /// Microchip Technology dsPIC30F
    EM_DSPIC30F = 118,
    /// Freescale Communication Engine RISC core
    EM_CE = 119,
    /// Renesas M32C series
    EM_M32C = 120,
    /// Altium TSK3000
    EM_TSK3000 = 131,
    /// FenTeC A32K
    EM_E2K = 132,
    /// Alpha 8-bit
    EM_TS11 = 133,
    /// STMicroelectronics ST100 (duplicate from earlier)
    EM_ST100_2 = 134,
    /// Xilinx MicroBlaze
    EM_MICROBLAZE = 189,
    /// AArch64 (64-bit ARM)
    EM_AARCH64 = 183,
    /// RISC-V
    EM_RISCV = 243,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Phdr {
    /// Segment type
    pub p_type: u32,
    /// Segment flags
    pub p_flags: u32,
    /// Segment file offset
    pub p_offset: u64,
    /// Segment virtual address
    pub p_vaddr: u64,
    /// Segment physical address
    pub p_paddr: u64,
    /// Segment file size
    pub p_filesz: u64,
    /// Segment memory size
    pub p_memsz: u64,
    /// Segment alignment
    pub p_align: u64,
}

#[repr(u32)]
#[derive(Debug, Copy, Clone, PartialEq)]
#[allow(non_camel_case_types)]
pub enum ElfPtType {
    /// Unused segment
    PT_NULL = 0,
    /// Loadable segment
    PT_LOAD = 1,
    /// Dynamic linking information
    PT_DYNAMIC = 2,
    /// Interpreter path
    PT_INTERP = 3,
    /// Auxiliary information
    PT_NOTE = 4,
    /// Unused
    PT_SHLIB = 5,
    /// Program header table itself
    PT_PHDR = 6,
    /// Thread-local storage
    PT_TLS = 7,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct ElfLoadInfo<'a> {
    /// Entry point address
    pub entry: u64,
    /// Number of loaded segments
    pub load_count: usize,
    /// Minimum virtual address
    pub min_vaddr: u64,
    /// Maximum virtual address
    pub max_vaddr: u64,
    /// Interpreter path (if PT_INTERP exists)
    pub interp_path: Option<&'a [u8]>,
}

pub struct ElfLoader;

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum ElfError {
    /// Invalid ELF format
    InvalidFormat,
    /// Invalid ELF header
    InvalidHeader,
    /// Not executable file
    NotExecutable,
    /// Machine type mismatch
    WrongMachine,
    /// Invalid program headers
    InvalidProgramHeaders,
    /// Out of memory
    OutOfMemory,
    /// Invalid segment
    InvalidSegment,
    /// No PT_LOAD segments
    NoLoadSegments,
}

// ---------------------------------------------------------------------------
// buffer: the page, the address space, the file chunk and the file buffer
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct Page {
    /// Page data
    pub data: Vec<u8>,
    /// Page status
    pub flags: AtomicUsize,
    /// Reference count
    pub ref_count: AtomicUsize,
}

pub struct AddressSpace {
    /// Page tree (simplified to array)
    /// Index is page number, value is page
    pub(super) pages: Spinlock<Vec<Option<Box<Page>>>>,
    /// File size (bytes)
    pub(super) size: AtomicUsize,
}

/// One fixed-size chunk: a page-allocation when the zone allocator can
/// serve it, a heap Vec as the pre-zone-init / fallback path.
pub(super) enum FileChunk {
    /// Physical address of an order-CHUNK_ORDER allocation.
    Page(usize),
    Heap(Vec<u8>),
}

pub struct FileBuffer {
    /// Logical length in bytes (chunks may hold zeros past it).
    pub(super) len: usize,
    /// Fixed-size chunks covering [0, chunks.len()*CHUNK). All chunks
    /// up to ceil(len/CHUNK) are allocated; a shrink keeps them until the
    /// trailing ones are dropped.
    pub(super) chunks: Vec<FileChunk>,
}

// ---------------------------------------------------------------------------
// bio: the buffer head and its state bits, the block cache with its cache
// entries, hash buckets and LRU state, MAX_WRITE_BATCH and the write
// batch family, and the publication family
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct BufferState(pub(super) u8);

pub struct BufferHead {
    /// Block device (typed handle; INV-BLKDEV-1 on blkdev::get_disk)
    pub b_device: Option<&'static blkdev::GenDisk>,
    /// Block number
    pub b_blocknr: u64,
    /// Block size
    pub b_size: u32,
    /// Buffer state
    pub b_state: Spinlock<BufferState>,
    /// Data
    pub b_data: Vec<u8>,
    /// Reference count
    pub(super) b_count: AtomicU32,
}

/// Cache entry wrapper for hash chaining and LRU list
pub(super) struct CacheEntry {
    /// The actual buffer head (raw pointer, owned by this entry)
    pub(super) bh: *mut BufferHead,
    /// Key: (device_major, device_minor, blocknr) for fast lookup.
    /// Minor included (review 5.3 low): two partitions of the same disk
    /// share a major number — without the minor, block N of partition A
    /// was served as block N of partition B.
    pub(super) key: (u32, u32, u64),
    /// Next entry in hash chain
    pub(super) hash_next: Option<*mut CacheEntry>,
    /// Previous entry in LRU list (more recent)
    pub(super) lru_prev: Option<*mut CacheEntry>,
    /// Next entry in LRU list (less recent)
    pub(super) lru_next: Option<*mut CacheEntry>,
    /// Eviction in progress (set under the bucket lock before the entry
    /// leaves the hash chain) — get() must skip such entries so no pin can
    /// arrive while the victim is being unlinked and freed (R7-C1).
    pub(super) evicting: bool,
}

/// A single hash bucket, protected by its own spinlock.
pub(super) struct HashBucket {
    /// Head of the hash chain
    pub(super) head: Option<*mut CacheEntry>,
}

/// Shared LRU list state (accessed under any bucket lock — each operation
/// touches only O(1) LRU nodes so contention is minimal).
pub(super) struct LruState {
    pub(super) head: Option<*mut CacheEntry>,
    pub(super) tail: Option<*mut CacheEntry>,
}

/// Block cache with per-bucket spinlock, global LRU, and atomic count.
///
/// # Lock hierarchy (MUST be followed to prevent deadlock)
///
/// 1. **Bucket lock** → 2. **LRU lock** (when both needed, always acquire
///    bucket lock first). LRU lock may be acquired alone (e.g. in evict_one).
/// 3. **BufferState lock** — per-buffer state, nested inside bucket lock.
///
/// No global mutex is held during I/O. Eviction syncs dirty buffers
/// *after* releasing the bucket lock.
pub struct BlockCache {
    /// Per-bucket hash chains, each with its own lock
    pub(super) buckets: Vec<Spinlock<HashBucket>>,
    /// Global LRU list (manipulated only while holding a bucket lock)
    pub(super) lru: Spinlock<LruState>,
    /// Global entry count (atomic — no lock needed to check capacity)
    pub(super) count: AtomicU32,
    /// Hash table size (must be power of 2)
    pub(super) hash_size: usize,
    /// Maximum entries (cache capacity)
    pub(super) max_entries: usize,
    /// Block size
    pub(super) block_size: u32,
}

/// In-flight cap of one WriteBatch window. Bounds the batch's private
/// memory (32 x 64 KiB spans = 2 MiB) and the device's pending-table
/// pressure (MAX_PENDING_IO_PCI = 64); the 32-submission safety-kick bound
/// of the PCI submit path matches it.
pub const MAX_WRITE_BATCH: usize = 32;

/// One inline batch slot: DMA source plus its completion (SUBMIT-2X:
/// the old shape Box-allocated the IoCompletion per span and pushed
/// both the entry and the results Vec — three global-allocator round
/// trips per 4 KiB direct write, ~half the write half's per-op delta
/// vs the read side, which already drains a stack completion array.
/// The inline slots cost a ~3 KiB zeroing construction, an order of
/// magnitude cheaper, and never touch the heap in the steady state).
pub(super) struct WriteSlot {
    /// Valid only while the slot is live (index < len): the span's DMA
    /// source. Reset to the empty Borrowed shape by flush().
    pub(super) data: WriteSource,
    /// Pre-armed completion; reset by flush() for reuse (the same
    /// discipline the direct-read batch's stack array follows).
    pub(super) completion: IoCompletion,
    /// Device-side start sector of this span (512-byte units) — the
    /// fusion key. Only meaningful on the deferred path.
    pub(super) sector: u64,
    /// Target device of this span — groups never fuse across devices.
    pub(super) dev: Option<&'static blkdev::GenDisk>,
}

/// A batch of asynchronously-submitted device writes (plugged writes).
///
/// Typical use (the run-batched flush's span path):
/// ```ignore
/// let mut batch = WriteBatch::new();
/// // ... per contiguous span:
/// if !batch.submit(dev, sector, span_bytes) {
///     // device without async write / window lag: drain + retry once,
///     // then fall back to blkdev_write for THIS span.
/// }
/// // ... after the last span:
/// batch.flush();
/// for i in 0..flushed { if batch.status(i) == 0 { /* clear BH_Dirty */ } }
/// ```
///
/// FUSION (the blk-merge front/back-merge shape, 2026-11-18): when the
/// device accepts SG requests, spans are NOT submitted at submit() time —
/// they accumulate in the window (the plug list) and flush() fuses every
/// run of device-adjacent spans (same device, sector_{i}+len_i ==
/// sector_{i+1}) into ONE scatter-gather chain: one request header, one
/// starting sector, N data descriptors, one completion — one device
/// round trip per fused group instead of one per span. Nothing is lost
/// by the deferral: chains are published QUIETLY and the device only
/// sees work at the kick, which has always happened at drain. Devices
/// without SG support keep the immediate per-span submission shape.
pub struct WriteBatch {
    /// Inline slots; entries are slots[0..len].
    pub(super) slots: [WriteSlot; MAX_WRITE_BATCH],
    /// Live entry count.
    pub(super) len: usize,
    /// Per-entry outcome of the LAST flush(), indexed by submission
    /// order. Valid for i < results_n; reset by the next flush.
    pub(super) results: [i32; MAX_WRITE_BATCH],
    pub(super) results_n: usize,
    /// Deferred (fusion) mode: decided by the FIRST submit from the
    /// device's SG capability. False → immediate per-span submission
    /// (the pre-fusion shape).
    pub(super) deferred: bool,
    /// Mode decided (first submit happened).
    pub(super) mode_set: bool,
}

/// DMA source of a batch entry: the batch's private copy (submit) or the
/// CALLER's buffer (submit_ref — paired-test item 1: the DIO write fast
/// path DMAs straight from the syscall's staging slice, saving one full
/// copy per span; the caller guarantees the bytes stay valid and
/// unmodified until the drain).
pub(super) enum WriteSource {
    Owned(alloc::boxed::Box<[u8]>),
    Borrowed {
        ptr: *const u8,
        len: usize,
    },
}

/// One deferred directory-entry publication (see the block comment above).
pub struct Publication {
    /// Monotonic id; the delete-side capture window addresses its record.
    pub id: u64,
    /// (major, minor) of the owning device.
    pub device: (u32, u32),
    /// Blocks that must persist BEFORE `entry` (create-side metadata).
    pub pre: Vec<u64>,
    /// The publishing directory block (None for post-only stragglers).
    pub entry: Option<u64>,
    /// Blocks that must persist AFTER `entry` (delete-side frees).
    pub post: Vec<u64>,
    /// Publishing context identity (the publishing task's address; 0 only
    /// for the no-task early-boot case). Scoped durability points flush
    /// only their own cookie's records — the leg-29A O(pending) fix: a
    /// whole-queue drain inside every fsync/fdatasync/mkdir made latency
    /// proportional to the WORLD's outstanding publications, and the
    /// drain's racing-publisher loop could pin an fsync for minutes under
    /// provisioning churn (sc310's 43-min hiview fdatasync).
    pub cookie: u64,
}

/// Leg-54 fix (the e3c1ec35 displacement, pa93j/pa95j evidence): the
/// pending-publication queue with a CONSTRAINED-BLOCK INDEX. The old
/// `Vec<Publication>` made `publication_constrained` — which runs at
/// EVERY out-of-band buffer write (sync_dirty_buffer, eviction victims,
/// journal commit write-through) — a linear scan of the whole queue and
/// each record's post list: with the post-mkdir flush deleted the queue
/// legitimately holds thousands of records during provisioning churn,
/// and every unlink's buffer writes paid O(queue x post-len) — p6_unlink
/// 10.2-10.8ms vs the 77-119us class. The index maps (device, block) ->
/// number of pending records constraining that block (entry == block or
/// post contains block, H1/H2's exact sets); membership checks are
/// O(log n). Mutations maintain it; wholesale take/requeue (the drain
/// snapshot pattern) rebuilds it once per requeue — drains are per
/// durability point, never per write.
#[allow(dead_code)]
pub struct PendingPublications {
    pub(super) list: Vec<Publication>,
    pub(super) index: alloc::collections::BTreeMap<((u32, u32), u64), u32>,
}

/// Ordered write plan for a scoped flush: `pre` blocks (no obligations of
/// their own), then `entry` blocks (each may complete pending records'
/// publications — their pres must already be in `pre`), then `post` blocks
/// (only the caller's own records' posts are ever scheduled). Blocks that
/// would land in two phases are written in the EARLIEST phase EXCEPT that
/// a block which is a pending `entry` anywhere in the plan is never
/// written in `pre` (it must follow its pres) — see `push_*` below.
pub(super) struct WritePlan {
    pub(super) pre: Vec<u64>,
    pub(super) entry: Vec<u64>,
    pub(super) post: Vec<u64>,
    /// All entry-role blocks of the CURRENT queue snapshot (drives the
    /// pre/entry exclusion rule while the plan is being built).
    pub(super) entry_role: Vec<u64>,
}

// ---------------------------------------------------------------------------
// vfs: the VFS path, the VFS state and its mount bucket, the filesystem
// type enum, the in-memory file content
// ---------------------------------------------------------------------------

/// VFS Path structure
///
/// Represents a resolved path with its mount and dentry information.
pub struct VfsPath {
    /// Dentry for this path
    pub dentry: Option<Arc<Dentry>>,
    /// Mount point (vfsmount)
    pub mnt: Option<*const u8>,
    /// Inode if resolved
    pub inode: Option<Arc<Inode>>,
}

/// VFS global state
pub(super) struct VfsState {
    /// Global VFS root dentry — the top of the dentry tree
    pub(super) root_dentry: Option<Arc<Dentry>>,
    pub(super) initialized: bool,
    /// Every live mount descriptor (dentry-tree mounts, not the string
    /// registry) — lets fd/dentry-based code find the mount governing a
    /// dentry WITHOUT reconstructing a path string (build_path() cannot
    /// cross mount boundaries: a mounted fs's root has no parent pointer).
    pub(super) mounts: Vec<Arc<VfsMountInternal>>,
    /// sc181: chain index — for every live mount, the PURE-PARENT chain of
    /// its mountpoint dentry ([mp, parents..., first parentless dentry])
    /// keys the mount into this map (key = dentry address, `anchor` pins
    /// it). The MS_REC bind clone scan used to walk the WHOLE registry,
    /// climbing parents for every mount (O(registry x depth) per bind,
    /// 10-25 ms and growing with the boot-mount count — ns.rs sc98; sc180
    /// telemetry: sandbox mount phases at 1.7-1.9 s). The index turns the
    /// query into a DFS touching only mounts actually nested under the
    /// bind source (collect_subtree_mounts).
    pub(super) mount_chain_index: BTreeMap<usize, MountBucket>,
}

/// One chain-index bucket. `anchor` pins the key dentry: without it a
/// freed intermediate chain dentry could have its address reused by a
/// fresh allocation, silently fusing two buckets.
pub(super) struct MountBucket {
    #[allow(dead_code)] // pins the key address; never dereferenced
    pub(super) anchor: Arc<Dentry>,
    /// Attach order (push order) — matches the registry Vec's order.
    pub(super) mounts: Vec<Arc<VfsMountInternal>>,
}

/// Filesystem type identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsType {    /// RootFS (memory-backed)
    RootFS,
    /// ext4 filesystem
    Ext4,
    /// procfs
    ProcFS,
    /// devfs
    DevFS,
    /// Unknown
    Unknown,
}

/// In-memory file content (stored in File's private_data)
pub(super) struct MemFileContent {
    pub(super) data: alloc::vec::Vec<u8>,
    pub(super) offset: usize,
    /// Dynamic-content hook (the procfs mount tables): regenerate
    /// `data` when the file is rewound — Linux seq_file semantics, where
    /// lseek(0) restarts the iterator and a re-read shows CURRENT state
    /// instead of the open-time snapshot. A plain fn pointer (no
    /// allocation): the generator reads the CURRENT task's mount
    /// namespace at call time, so the rewind view is the reader's own.
    pub(super) regen: Option<fn() -> alloc::vec::Vec<u8>>,
}

// ---------------------------------------------------------------------------
// path: the namei data, the path component, the path and its iterator
// ---------------------------------------------------------------------------

#[repr(C)]
pub struct NameiData<'a> {
    /// Current position
    pub path: Path<'a>,
    /// Last component
    pub last: Option<PathComponent<'a>>,
    /// Lookup flags
    pub flags: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct PathComponent<'a> {
    /// Component name
    pub name: &'a str,
    /// Component length
    pub len: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct Path<'a> {
    /// Path string
    pub path: &'a str,
}

pub struct PathComponents<'a> {
    /// Path string
    pub(super) path: &'a str,
    /// Current position
    pub(super) pos: usize,
}

// ---------------------------------------------------------------------------
// superblock: the superblock flags, the superblock, the mount-callback
// context (FsContext), the filesystem type and the registry
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct SuperBlockFlags(pub(super) u64);

#[repr(C)]
pub struct SuperBlock {
    /// Filesystem flags
    pub s_flags: SuperBlockFlags,
    /// Block size
    pub s_blocksize: usize,
    /// Block size bits
    pub s_blocksize_bits: u8,
    /// Filesystem magic number
    pub s_magic: u32,
    /// Maximum links
    pub s_max_links: u32,
    /// Root inode
    pub s_root: Option<Arc<()>>,
    /// Filesystem type
    pub s_type: Option<&'static FileSystemType>,
    /// Mount options
    pub s_options: Option<Arc<()>>,
    /// Private data (for specific filesystem)
    pub s_fs_info: Option<*mut u8>,
}

pub struct FsContext<'a> {
    /// Source device
    pub source: Option<&'a str>,
    /// Mount target
    pub target: Option<&'a str>,
    /// Mount flags
    pub ms_flags: u64,
    /// Data options
    pub data: Option<&'a str>,
}

/// The mount-callback table (round-5, L5): the pointers are SAFE fns —
/// registration is the audited edge (the same de-sign the FileOps table
/// already carries; the `extern "C"` was cargo cult — no C caller
/// exists). The `*mut SuperBlock` handle keeps its documented raw
/// contract until the mount layer adopts typed handles.
#[repr(C)]
pub struct FileSystemType {
    /// Filesystem name
    pub name: &'static str,
    /// Get superblock (called on mount)
    pub mount: Option<fn(&FsContext<'_>) -> Result<*mut SuperBlock, i32>>,
    /// Kill superblock (called on unmount)
    pub kill_sb: Option<fn(*mut SuperBlock)>,
    /// Filesystem flags
    pub fs_flags: u64,
}

pub(super) struct FsRegistry {
    /// Filesystem type list
    pub(super) fs_types: Spinlock<[Option<&'static FileSystemType>; 32]>,
}

// ---------------------------------------------------------------------------
// mount: the mount flags and the VFS mount
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct MntFlags(pub(super) u64);

/// Per-mount metadata (used by individual filesystems for local state).
#[repr(C)]
pub struct VfsMount {
    pub mnt_id: u64,
    pub mnt_flags: MntFlags,
    pub mnt_mountpoint: Option<Arc<Vec<u8>>>,
    pub mnt_root: Option<Arc<Vec<u8>>>,
    pub mnt_sb: Option<*mut u8>,
    pub(super) mnt_count: core::sync::atomic::AtomicU64,
    pub(super) mnt_expired: core::sync::atomic::AtomicU64,
}

// ---------------------------------------------------------------------------
// mount_table: the mount-callback signature and a table row
// ---------------------------------------------------------------------------

/// One leaf's mount entry point: performs the instance creation and
/// `vfs_mount`, and registers the mount row (the old match-arm body).
pub type MountFn = fn(source: &str, target: &str, fs_type: &str, data: Option<&str>, flags: MntFlags) -> Result<(), i32>;

pub(super) struct Entry {
    pub(super) names: &'static [&'static str],
    pub(super) f: MountFn,
}

// ---------------------------------------------------------------------------
// leaf_glue: the tmpfs tag kind and the input-device registration
// ---------------------------------------------------------------------------

/// The fsctx mount-instance tag: devpts/securityfs/mqueue are tmpfs
/// instances whose fresh root is retagged with a filesystem-type ops
/// twin (the statfs magic dispatch); `None` tags nothing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TmpfsTagKind {
    None,
    Devpts,
    Securityfs,
    Mqueue,
}

/// The input-device registration record (evdev -> sysfs uevent + tree).
/// Framework-owned DTO: the drivers fill it, the fs sysfs leaf
/// consumes it through the populate row (plain data — no leaf types cross
/// the boundary). The uevent environment block builder lives with it.
#[derive(Debug, Clone)]
pub struct InputEventReg {
    /// input_dev kobject name ("input0")
    pub input_name: String,
    /// event node name ("event0")
    pub event_name: String,
    pub major: u32,
    pub minor: u32,
    /// input device name ("VirtIO Keyboard") — NAME= env + name attr
    pub dev_name: String,
    pub bustype: u32,
    pub vendor: u32,
    pub product: u32,
    pub version: u32,
    /// EV= bitmap string (Linux input_print_bitmap format: space-separated
    /// lowercase-hex 64-bit words, most-significant first)
    pub ev: String,
    /// KEY= bitmap string, empty -> line omitted (type not supported)
    pub key: String,
    /// REL= bitmap string, empty -> omitted
    pub rel: String,
    /// ABS= bitmap string, empty -> omitted
    pub abs: String,
}

// ---------------------------------------------------------------------------
// perf_ctr: the per-CPU time accounting record
// ---------------------------------------------------------------------------

/// One CPU's time accounting, in jiffies (Linux /proc/stat columns).
///
/// `nice`, `iowait`, `irq`, `softirq`, `steal`, `guest` and
/// `guest_nice` exist because Linux's file has those columns; Rux has
/// no nice levels, no iowait accounting and no IRQ-time accounting
/// (CONFIG_IRQ_TIME_ACCOUNTING is off on many Linux boxes too), so
/// they report zero rather than a fabricated number.
pub struct CpuTime {
    pub user: AtomicU64,
    pub nice: AtomicU64,
    pub system: AtomicU64,
    pub idle: AtomicU64,
    pub iowait: AtomicU64,
    pub irq: AtomicU64,
    pub softirq: AtomicU64,
    pub steal: AtomicU64,
    pub guest: AtomicU64,
    pub guest_nice: AtomicU64,
}

// ---------------------------------------------------------------------------
// stat: the stat record, its x86_64 layout, the statx record and timestamp
// ---------------------------------------------------------------------------

/// File status information
///
/// ...
///
/// Layout matches musl libc for RISC-V 64-bit:
/// - st_dev: u64 @ 0
/// - st_ino: u64 @ 8
/// - st_mode: u32 @ 16
/// - st_nlink: u32 @ 20
/// - st_uid: u32 @ 24
/// - st_gid: u32 @ 28
/// - st_rdev: u64 @ 32
/// - __pad: u64 @ 40
/// - st_size: i64 @ 48
/// - st_blksize: i64 @ 56 (musl uses long, not int)
/// - st_blocks: i64 @ 64
/// - st_atime: i64 @ 72
/// - st_atime_nsec: u64 @ 80
/// - st_mtime: i64 @ 88
/// - st_mtime_nsec: u64 @ 96
/// - st_ctime: i64 @ 104
/// - st_ctime_nsec: u64 @ 112
/// - __unused: [u64; 2] @ 120 (padding to 128 bytes)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Stat {
    /// Device ID (st_dev)
    pub st_dev: u64,

    /// Inode number (st_ino)
    pub st_ino: u64,

    /// File type and permissions (st_mode)
    pub st_mode: u32,

    /// Hard link count (st_nlink)
    pub st_nlink: u32,

    /// User ID (st_uid)
    pub st_uid: u32,

    /// Group ID (st_gid)
    pub st_gid: u32,

    /// Device ID (if special file) (st_rdev)
    pub st_rdev: u64,

    /// Padding (musl expects gap here)
    pub(super) __pad1: u64,

    /// File size (bytes) (st_size)
    pub st_size: i64,

    /// Block size (st_blksize) - musl uses long (64-bit on riscv64)
    pub st_blksize: i64,

    /// Number of 512-byte blocks allocated (st_blocks)
    pub st_blocks: i64,

    /// Last access time (st_atime)
    pub st_atime: i64,

    /// Nanoseconds part of last access time (st_atime_nsec)
    pub st_atime_nsec: u64,

    /// Last modification time (st_mtime)
    pub st_mtime: i64,

    /// Nanoseconds part of last modification time (st_mtime_nsec)
    pub st_mtime_nsec: u64,

    /// Last status change time (st_ctime)
    pub st_ctime: i64,

    /// Nanoseconds part of last status change time (st_ctime_nsec)
    pub st_ctime_nsec: u64,

    /// Unused padding to match 128 byte struct size
    pub(super) __unused: [u32; 2],
}

/// x86_64 glibc `struct stat` — the x86 ABI does NOT use the asm-generic
/// layout above: nlink is 64-bit and precedes mode, there is a pad after
/// gid and three trailing reserved words (144 bytes total). musl on
/// riscv64 and glibc-on-x86 read different offsets for the same field,
/// so copying the generic struct made every x86_64 stat() see scrambled
/// fields (st_mode read st_uid → toybox ls listed directories as plain
/// files).
#[cfg(feature = "x86_64")]
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct X86Stat {
    pub st_dev: u64,
    pub st_ino: u64,
    pub st_nlink: u64,
    pub st_mode: u32,
    pub st_uid: u32,
    pub st_gid: u32,
    pub(super) __pad0: u32,
    pub st_rdev: u64,
    pub st_size: i64,
    pub st_blksize: i64,
    pub st_blocks: i64,
    pub st_atime: i64,
    pub st_atime_nsec: i64,
    pub st_mtime: i64,
    pub st_mtime_nsec: i64,
    pub st_ctime: i64,
    pub st_ctime_nsec: i64,
    pub(super) __glibc_reserved: [i64; 3],
}

/// Extended file status information (statx)
///
/// struct statx layout for RISC-V 64-bit (256 bytes).
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Statx {
    /// Mask of what was filled in (STATX_*)
    pub stx_mask: u32,
    /// Preferred block size for I/O
    pub stx_blksize: u32,
    /// Mount-specific flags
    pub stx_attributes: u64,
    /// Number of hard links
    pub stx_nlink: u32,
    /// User ID
    pub stx_uid: u32,
    /// Group ID
    pub stx_gid: u32,
    /// File mode (type + permissions)
    pub stx_mode: u16,
    pub(super) __spare0: u16,
    /// Inode number
    pub stx_ino: u64,
    /// File size (bytes)
    pub stx_size: u64,
    /// Number of 512-byte blocks allocated
    pub stx_blocks: u64,
    /// Mask for stx_attributes
    pub stx_attributes_mask: u64,
    /// Last access time
    pub stx_atime: StatxTimestamp,
    /// Creation time (btime)
    pub stx_btime: StatxTimestamp,
    /// Last status change time
    pub stx_ctime: StatxTimestamp,
    /// Last modification time
    pub stx_mtime: StatxTimestamp,
    /// Device major for special files
    pub stx_rdev_major: u32,
    /// Device minor for special files
    pub stx_rdev_minor: u32,
    /// Device major of filesystem
    pub stx_dev_major: u32,
    /// Device minor of filesystem
    pub stx_dev_minor: u32,
    /// Mount ID of the mount the file is on (STATX_MNT_ID, Linux
    /// offset 0x90). Same id space as /proc/self/mountinfo's first
    /// column — systemd 255's mount detection compares a path's
    /// stx_mnt_id with its parent's (path_is_mount_point) and matches
    /// device nodes against mountinfo rows by mnt_id.
    pub stx_mnt_id: u64,
    /// Spare space to reach 256 bytes
    pub(super) __spare2: [u64; 13],
}

/// Timestamp for statx
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct StatxTimestamp {
    /// Seconds since epoch
    pub tv_sec: i64,
    /// Nanoseconds
    pub tv_nsec: u32,
    /// Reserved
    pub(super) __reserved: i32,
}

// ---------------------------------------------------------------------------
// dev_t: the device number
// ---------------------------------------------------------------------------

/// Device number (major:minor)
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct DevNo {
    /// Major device number
    pub major: u32,
    /// Minor device number
    pub minor: u32,
}

// ---------------------------------------------------------------------------
// fs_struct: the per-process filesystem information (root, cwd, umask)
// ---------------------------------------------------------------------------

/// Filesystem information structure
///
/// Contains per-process filesystem context that can be shared
/// between threads when CLONE_FS is used.
pub struct FsStruct {
    /// Current working directory path
    pub(super) cwd: RwSpinlock<alloc::vec::Vec<u8>>,

    /// Root directory path (for chroot)
    pub(super) root: RwSpinlock<alloc::vec::Vec<u8>>,

    /// The cwd as a dentry (mount-aware — the dentry reached after
    /// follow_mount crossings). Linux keeps fs->pwd as a (vfsmount,
    /// dentry) pair and derives every path string from it on demand;
    /// the string above is Rux's lookup-prefix equivalent. Strings go
    /// STALE when mounts move (MS_MOVE): a cwd string captured as
    /// "/usr" keeps naming the empty dir after the /usr mount moved
    /// to "/" (the OH switch_root failure). The dentry follows the
    /// mount (its mountpoint back-edge is updated by the move), so it
    /// stays valid — getters below REGENERATE the string from the
    /// dentry when one is recorded.
    pub(super) cwd_dentry: RwSpinlock<Option<Arc<Dentry>>>,

    /// The chroot jail root as a dentry (None = not chrooted / the
    /// global root). Same self-healing rationale as cwd_dentry.
    pub(super) root_dentry: RwSpinlock<Option<Arc<Dentry>>>,

    /// put_old recorded by this process's last pivot_root(2) (jail-style
    /// pivot stacks no real mount — see sys_pivot_root). The post-pivot
    /// cleanup umount2(put_old | ".", MNT_DETACH) consumes it: Linux
    /// leaves the OLD root mounted at put_old and that umount drops it;
    /// here it just reports success for the matching path once.
    pub(super) pivot_put_old: RwSpinlock<Option<alloc::vec::Vec<u8>>>,

    /// True when the current root was installed by pivot_root(2) or by
    /// setns(2) entering a pivoted mount namespace (NOT by plain chroot).
    /// setns back into a non-pivoted namespace must then reset the root
    /// to that ns's root — the pivot's whole purpose is to be undone by
    /// the namespace jump (OH beget: pivot into the sandbox, setns back
    /// to the default ns for the rest of the boot).
    pub(super) root_is_pivot: RwSpinlock<bool>,

    /// File creation mask
    pub(super) umask: AtomicU32,
}

// ---------------------------------------------------------------------------
// page_cache: the cached page, the batch-fill and dirty-user outcomes,
// the reader bank, the slot table and the constants it is sized by,
// the parked batch, the per-inode page cache, the page cache,
// the frame window
// ---------------------------------------------------------------------------

/// Fill source for the batch inserters: a kernel slice (read-ahead fills,
/// staged writes) or USER memory (the buffered-write(2) fast path, BUFW-2X:
/// bytes are copied straight into the fresh frames — ONE copy per page —
/// and the syscall layer's staging buffer with its second copy is skipped).
pub(super) enum BatchFill<'a> {
    Slice(&'a [u8]),
    /// access_ok-validated user pointer + total length.
    User { ptr: usize, len: usize },
}

/// Outcome of [`PageCache::insert_dirty_user`] — the verdict the per-page
/// buffered-write loop branches on. FAULT must be distinguishable from
/// CONTENTION: a fault ENDS the write (short-write/EFAULT semantics), a
/// contention just re-loops through pin-and-copy.
pub enum DirtyUserOutcome {
    /// Frame installed born-dirty (or a retired squatter revived with the
    /// user page's bytes) — the caller counts the page as copied.
    Installed,
    /// Duplicate fresh entry / table full / OOM — the caller re-loops
    /// through the pin-and-copy path, exactly like `insert_dirty`
    /// returning false.
    Contention,
    /// The user copy faulted before any byte could be counted — the
    /// write stops here.
    Fault,
}

/// A cached page of file data, backed by a zone-allocated physical page frame.
pub(super) struct CachedPage {
    /// Physical frame number (allocated from zone allocator).
    pub(super) pfn: usize,
    /// Reference count counting EXTERNAL pins (get..put windows). Pages are
    /// born at 0 — the cache's own ownership is not counted. (R7-C2: birth
    /// ref 1 with no matching put pinned every page forever, disabling
    /// eviction and making invalidate keep stale data.)
    pub(super) ref_count: AtomicU32,
    /// Set when invalidate_inode found the page pinned: freed by the final
    /// put() instead of under the reader's feet.
    pub(super) invalidated: bool,
    /// R14-17: one-shot release latch — put() and the evict/shrink paths
    /// can both reach the release for the same entry; the frame must be
    /// returned to the zone exactly once (the zone dblfree stream).
    pub(super) released: core::sync::atomic::AtomicBool,
    /// Session 25 (MAP_SHARED writeback): the frame holds stores from a
    /// writable shared mapping that have NOT reached the file yet. Set
    /// conservatively at W-map install time (a writable mapping MAY write;
    /// flushing an unwritten page is harmless I/O), cleared by the
    /// msync/fsync flush. Dirty entries skip eviction — their content
    /// exists nowhere else but the frame.
    pub(super) dirty: core::sync::atomic::AtomicBool,
}

/// Number of per-CPU reader banks (pcache Option B stage 1). Kernel.toml's
/// SMP ceiling is 4; 8 leaves headroom. A cpu id at or above this bound
/// falls back to the slow-path lock (the r8final discipline — bank
/// selection happens with interrupts off, so the id is stable). Same
/// shape as FdTable's banks (8.10.22).
pub(super) const PC_READER_BANKS: usize = 8;

/// One per-CPU reader-nesting counter, padded to its own cacheline so
/// the CPUs' counter RMWs never share a line.
#[repr(align(64))]
pub(super) struct PcReaderBank(pub(super) core::sync::atomic::AtomicUsize);

/// Fixed-capacity inode-entry slot table (2^15 slots = 256KB of .bss).
/// Load factors in practice are far below 10% (entries live only while
/// their inode has cached pages), so linear-probe chains stay 1-3 slots.
pub(super) const PC_TABLE_CAP: usize = 32768;
pub(super) const PC_TABLE_MASK: usize = PC_TABLE_CAP - 1;

/// One open-addressed slot: the Arc<InodePageCache> as a raw pointer.
/// null = never used (probe stop); PC_TOMBSTONE_BITS = entry removed
/// (probe continues — tombstoning instead of nulling keeps the OTHER
/// entries' probe chains intact). Single-word aligned stores, so a
/// lock-free load can never observe a torn value.
pub(super) struct PcSlot(pub(super) core::sync::atomic::AtomicPtr<InodePageCache>);

/// Parked-Arc batch carried out of a wlock section. Eviction may empty
/// entries; their slot references are parked into this fixed array under
/// wlock (Vec push would allocate — never under a pcache lock) and moved
/// into `retired` + drained right after the wlock drops.
pub(super) const PC_PARK_BATCH: usize = 8;

pub(super) type PcParkBatch = [Option<Arc<InodePageCache>>; PC_PARK_BATCH];

/// Per-inode page cache: the lock-free lookup unit.
pub struct InodePageCache {
    /// Identity for the slot-table probe compare (== cache_key). Written
    /// once at construction, never mutated — lock-free readers compare it
    /// inside their claim window.
    pub(super) key: u64,
    /// Per-inode lock (Option B stage 1): the guard IS the &mut access to
    /// the pages map (every CachedPage field is reached through it — the
    /// leg-54 locked-plain discipline transfers verbatim). Nesting:
    /// `wlock → this` only, never the reverse.
    pub(super) lock: Spinlock<BTreeMap<u64, CachedPage>>,
    /// Identity of the owning (filesystem, inode) — cache_key is a
    /// non-reversible hash, so reclaim-time writeback (shrink_reclaim)
    /// records the raw inputs here to resolve a live VFS Inode via
    /// icache_lookup and flush dirty entries through the normal
    /// ext4_flush_dirty_inode machinery.
    pub(super) fs_id: u64,
    pub(super) ino: u64,
    /// Delayed-allocation EOF watermark (2026-11): the highest byte
    /// offset a buffered write published while its tail pages were still
    /// dirty. The revival guard needs the EXACT EOF — a page-rounded
    /// extent would report 4096 for a 1000-byte appended file and hand
    /// readers three pages of phantom zeros. CAS-max at publish, never
    /// lowered: a stale-high value is always clamped by the surviving
    /// dirty extent at read time (`dirty_tail_eof`), and any prefix of
    /// it that reached writeback is covered by the on-disk size.
    pub(super) dirty_eof: core::sync::atomic::AtomicU64,
}

/// Global page cache, keyed by the combined (fs_id, inode) cache key.
///
/// The inode map is the Option-B slot table; see the module LOCKING note.
pub struct PageCache {
    /// Lock-free inode-entry slot table (readers: bank claim + Acquire
    /// loads; writers: wlock + Release stores).
    pub(super) slots: [PcSlot; PC_TABLE_CAP],
    /// Per-CPU reader-nesting banks (see `lookup_claimed` for the protocol).
    pub(super) readers: [PcReaderBank; PC_READER_BANKS],
    /// Writer-side table structure lock: entry publish/remove, eviction,
    /// whole-table iteration. Ordering: wlock → per-inode lock, never
    /// reverse. No allocation under it.
    pub(super) wlock: Spinlock<()>,
    /// Whole entries removed from the table, parked until a drain can
    /// drop them with no lock-free reader mid-claim (FdTable.retired).
    pub(super) retired: Spinlock<alloc::vec::Vec<Arc<InodePageCache>>>,
    /// Slow-path exclusion for readers whose cpu id falls outside the
    /// bank range (r8final family); drains hold it across the drop.
    pub(super) slow_read: Spinlock<()>,
    /// Total number of cached pages (for global limit).
    pub(super) total_pages: AtomicU32,
}

/// A byte window over one exclusively-owned page frame (INV-PC-WIN):
/// `phys` names a frame the CALLER owns for this call (zone-allocated,
/// not yet in the cache/map); the window is exactly PAGE_SIZE bytes of
/// the linear map and lives only for the call. The one audited edge in
/// this file for frame fills/copies — the four INV-PC-PAGECOPY sites
/// below route through it.
pub(super) struct FrameWindow {
    pub(super) ptr: *mut u8,
}

// ---------------------------------------------------------------------------
// readahead: the read-ahead state
// ---------------------------------------------------------------------------

/// Read-ahead state for a single open file descriptor.
pub struct ReadAheadState {
    /// Offset where the last read ended.
    pub last_read_end: u64,
    /// Number of consecutive sequential reads detected.
    pub sequential_count: u32,
    /// Whether read-ahead is currently active.
    pub active: bool,
    /// Page index up to which read-ahead has been issued.
    pub ra_until: u64,
    /// Filesystem block size (cached at open time).
    pub block_size: u64,
}

// ---------------------------------------------------------------------------
// io_completion: the I/O completion signal
// ---------------------------------------------------------------------------

/// I/O completion signal.
///
/// # Usage
/// ```ignore
/// let comp = IoCompletion::new();
/// // submit async I/O with &comp ...
/// let status = comp.wait();  // blocks until I/O finishes
/// ```
///
/// Thread-safe: `complete()` is called from interrupt context,
/// `wait()` from any kernel task.
pub struct IoCompletion {
    /// True when the I/O has finished.
    pub(super) done: AtomicBool,
    /// 0 = success, negative = errno (e.g. -EIO).
    pub(super) status: AtomicI32,
    /// In-flight delivery count (the walker-vs-abandon race fix).
    ///
    /// A completion walker takes a pending entry out of the device table
    /// under the table lock but DELIVERS (calls [`Self::complete`]) after
    /// dropping it. A waiter whose 10s deadline expires concurrently
    /// retires its entries via `abandon_pending_completion` — which scans
    /// the tables under the same locks and cannot see the already-taken
    /// entry. Without a handshake the waiter would return -ETIMEDOUT, its
    /// kernel-stack `IoCompletion` would be recycled, and the late
    /// `complete()` would fire a wake_up_all into freed stack memory —
    /// the wandering corruption behind ftest01's cross-file bad-verify.
    ///
    /// Protocol: the walker increments before dropping the table lock
    /// ([`Self::begin_delivery`]) and decrements after `complete()`
    /// ([`Self::end_delivery`]); `wait()` drains the count to zero before
    /// every return (its frame — and the completion with it — often dies
    /// at return), and abandon drains it before reporting -ETIMEDOUT.
    pub(super) delivering: AtomicUsize,
    /// Device-truth token of the delivery that set `done` (the
    /// zero-serve guard): the DMA data-buffer pointer the DRIVER recorded
    /// in the pending entry, stamped by [`Self::complete_with_token`].
    ///
    /// 0 = the signal came from an internal path (bio cache hit,
    /// in-flight branch, dup-path re-settle) that did not go through a
    /// device completion walker. `wait()` drains deliveries before
    /// returning, so a subsequent [`Self::delivered_token`] load is
    /// stable. The waiter (bread_wait) compares the token against its own
    /// buffer: a mismatch means the walker delivered SOME OTHER chain's
    /// completion — the buffer's DMA may not have landed, so the waiter
    /// must NOT treat the data as valid (the no-journal ext4 root-lookup
    /// zero-serve family).
    pub(super) token: AtomicUsize,
    /// Tasks sleeping for completion.
    pub(super) wait_queue: WaitQueueHead,
}

// ---------------------------------------------------------------------------
// locks: the lock owner and the conflict report
// ---------------------------------------------------------------------------

/// Record-lock ownership token mirror (the tables storing it live in
/// the fs crate's locks module).
///
/// `owner` mirrors Linux's `fl_owner` token: `Pid` for classic POSIX
/// locks (all threads of one process share them), `Ofd(file_id)` for
/// open-file-description locks (F_OFD_SETLK — one description per
/// `File::file_id`). Two locks never conflict when their owner tokens
/// are EQUAL, regardless of kind — exactly Linux's `posix_locks_conflict`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum LockOwner {
    Pid(u32),
    Ofd(u64),
}

/// A conflicting lock reported to F_GETLK (constructed crate-side from
/// the posix_test_lock row, consumed by the VFS fcntl arm).
pub struct LockConflict {
    pub exclusive: bool,
    pub start: u64,
    pub end: u64,
    pub pid: u32,
}

// ---------------------------------------------------------------------------
// vt: the vt_mode record
// ---------------------------------------------------------------------------

/// struct vt_mode (8 bytes): mode, waitv, relsig, acqsig, frsig.
pub(crate) struct VtMode {
    pub(crate) mode: u8,
    pub(crate) waitv: u8,
    pub(crate) relsig: u16,
    pub(crate) acqsig: u16,
    pub(crate) frsig: u16,
}

// ---------------------------------------------------------------------------
// pidfd: the pidfd payload
// ---------------------------------------------------------------------------

/// The pinned identity carried by every pidfd-shaped file.
pub struct PidFdData {
    /// The process id (thread-group id) at open time.
    pub pid: u32,
    /// PINNED thread-group leader. Valid until `close` drops the pin;
    /// never dereferenced beyond pid/state/cred reads.
    pub task: *mut Task,
}

// ---------------------------------------------------------------------------
// fsctx: the new mount API's context state machine and the detached mount object
// ---------------------------------------------------------------------------

/// The fs_context state machine (fs/fs_context.c). Only the states the
/// create pipeline needs are modelled: parameters before CREATE, a root
/// instance after it. `reconfigure` marks an fspick() context (purpose
/// FS_CONTEXT_FOR_RECONFIGURE): parameters are accepted until
/// FSCONFIG_CMD_RECONFIGURE (a no-op — the live mount IS the state),
/// CREATE/fsmount are refused like the reference's phase checks.
pub struct FsConfigContext {
    pub fs_type: String,
    /// The "source" parameter (fsconfig SET_STRING "source") — the
    /// device/name shown in /proc/mounts.
    pub source: Option<String>,
    /// Recorded key[=value] parameters, in fsconfig order.
    pub params: Vec<(String, Option<String>)>,
    /// Monolithic option string for legacy types (joined like
    /// legacy_parse_param's buffer).
    pub legacy_data: String,
    /// sb_flags spellings ("ro" flag) recorded before CREATE.
    pub sb_readonly: bool,
    /// Root instance after FSCONFIG_CMD_CREATE (fc->root).
    pub root: Option<Arc<Inode>>,
    /// Effective read-only after CREATE (ext4 metadata_csum downgrades).
    pub effective_ro: bool,
    /// fspick() product: reconfigure-purpose context.
    pub reconfigure: bool,
    /// fspick() product: the live mount CMD_RECONFIGURE commits onto.
    pub target_mount: Option<Arc<VfsMountInternal>>,
}

/// A mount fd's payload: the (possibly detached) VfsMountInternal plus
/// the bookkeeping /proc/mounts needs when it gets attached.
pub struct MountFd {
    pub mnt: Arc<VfsMountInternal>,
    /// Attached into the dentry tree already (move_mount happened)?
    pub attached: bool,
    /// For the /proc/mounts row at attach time.
    pub fs_type: String,
    pub source: String,
    /// open_tree(AT_RECURSIVE|OPEN_TREE_CLONE) submount clones, recorded
    /// at clone time and materialized INSIDE the mount root at
    /// move_mount-attach time (the MS_REC bind clone discipline: the
    /// deferred roots share the source mounts' inodes, same superblock).
    pub submounts: Vec<(Vec<String>, Arc<Dentry>, MntFlags)>,
    /// The ORIGINAL subtree root the submount rel-paths are relative to
    /// (open_tree's resolved source `anc`). The materialization walk's
    /// source-side cursor starts here — kernel-created mountpoints
    /// (/proc, /dev, /sys: dcache-only, no on-disk entries) resolve
    /// through it when the fresh root's disk tree has no placeholders
    /// (same fallback as the MS_REC bind clone, flip6).
    pub submount_src_base: Option<Arc<Dentry>>,
}

// ---------------------------------------------------------------------------
// journal_seam: the journal capture state and the asserting Arc wrapper
// ---------------------------------------------------------------------------

/// The deferred-publication capture window state (see
/// bio::PENDING_PUBLICATIONS). While armed in PRE mode (create-side
/// operations: mkdir/create/symlink/link/rename-add), every buffer
/// marked dirty on the capture device is recorded on `pre_list`; the
/// list becomes the `pre` set of the publication created at the
/// directory-entry write (an exact snapshot of THIS operation's own
/// metadata — the earlier device-wide dirty snapshot cost O(cache) per
/// op and O(N^2+) across a create-heavy loop such as LTP creat05/
/// fork09, which create 1021 files in setup). While armed in POST mode
/// (unlink/rmdir/rename-replace) buffers marked dirty are appended to
/// the `post` set of publication `post_id` instead. One mode at a time.
pub struct JournalCaptureState {
    /// 0=off 1=pre 2=post (CAPTURE_* above).
    pub mode: Cell<u8>,
    /// POST-mode target publication id.
    pub post_id: Cell<u64>,
    /// The capture device (major, minor).
    pub dev_key: Cell<(u32, u32)>,
    /// PRE-mode capture accumulator (task-local: only the owning
    /// task's syscalls append, via the dirty-mark hook).
    pub pre_list: crate::sync::spinlock::Spinlock<alloc::vec::Vec<u64>>,
}

/// Ext4FileSystem's journal slot — an ASSERTING WRAPPER (the type's
/// raw-pointer protocol state stays crate-side; the Send/Sync
/// assertion is journal-protocol-level and lives HERE, framework-side,
/// like the lock above).
///
/// INV-JSEAM-4: the wrapped `Arc<Journal>` is shared across threads
/// exactly as before (task contexts + the commit kthread); every raw
/// pointer inside Journal is dereferenced only under the journal's own
/// spinlocks (the jbd2 protocol), so sharing the Arc is sound. The
/// wrapper exists because the crate cannot carry the `unsafe impl`.
pub struct AssertingSyncArc<T>(pub Option<alloc::sync::Arc<T>>);
