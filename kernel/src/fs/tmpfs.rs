//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! tmpfs — page-granular in-memory filesystem (P0-6)
//!
//! Backing store for `/dev/shm` (POSIX `shm_open` via musl maps it to
//! `/dev/shm/<name>`) and general RAM-backed mounts (`mount -t tmpfs`).
//!
//! Storage model:
//! - File contents live in a `BTreeMap<usize, Box<[u8; 4096]>>` — pages are
//!   allocated ONLY when written; reads of never-written pages return zeros
//!   (sparse semantics, like Linux shmem). A file's logical size is tracked
//!   separately from its page set, so `truncate` + re-extend yields zeros.
//! - Pages are kernel-heap allocations. They are NOT refcounted into RSS and
//!   are NEVER reclaimed by kswapd (no rmap / no swap backing). They are
//!   freed on unlink/last-close-of-truncated-size/truncate. Documented
//!   limitation of this wave; a shmem inode cache + reclaim path is future
//!   work.
//! - Hard links share the SAME `Arc<Spinlock<BTreeMap>>` (POSIX
//!   shared-visibility semantics, same design as rootfs).
//!
//! Instances:
//! - Every `mount -t tmpfs` creates an INDEPENDENT `TmpfsSuperBlock` (own
//!   inode number space and own icache `fs_id` — low bits of the id are the
//!   instance counter, so (ino, fs_id) never collide across mounts).
//! - Superblocks are leaked on mount (mount-lifetime objects, like the ext4
//!   global instance); umount detaches the dentry but does not free the
//!   tree (open inodes keep their `Arc<TmpfsNode>`s alive regardless).
//!
//! Unsupported (documented): mmap of tmpfs files (files are not backed by
//! refcounted user pages — mapping falls back to the generic inode path),
//! memory limits/statfs accounting is static.

use crate::errno;
use crate::fs::inode::{file_type, setattr_attr, Inode, InodeMode, Ino, INodeOps, VfsDirEntry};
use crate::fs::superblock::{FileSystemType, FsContext, SuperBlock, SuperBlockFlags};
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use crate::sync::spinlock::Spinlock;

/// Linux TMPFS_MAGIC.
pub const TMPFS_MAGIC: u32 = 0x0102_1994;

/// tmpfs page size (matches the kernel page size / stat blksize).
const PAGE_SIZE: usize = 4096;

/// icache identity prefix ("TPFS"); the low bits carry the per-mount
/// instance id so (ino, fs_id) pairs from different tmpfs mounts never
/// collide (VFS-H8 discipline — see FS_ID_ROOTFS in inode.rs).
const FS_ID_TMPFS_BASE: u64 = 0x5450_4653_0000_0000;

/// Wall-clock seconds since epoch (RTC-armed boot offset + monotonic);
/// tmpfs atime/mtime are Unix epoch time, as on Linux.
fn wall_secs() -> u64 {
    crate::drivers::rtc::wall_secs()
}

/// One 4 KiB data page of a tmpfs file, plus the owning superblock.
///
/// Drop releases the page back to the mount's page budget
/// (`TmpfsSuperBlock::used_pages`), so EVERY removal path — truncate,
/// unlink, hard-link-drop, whole-mount teardown — decrements exactly
/// once, by construction. The size= budget is what keeps an
/// unprivileged `mount -t tmpfs -o size=32M` from eating the whole
/// kernel heap: tmpfs pages are kernel-heap boxes here (no swap, no
/// page-cache reclaim), so without the limit a 118 MB write pattern
/// (LTP preadv203) exhausts the 128 MB heap and the ALLOCTHROW path
/// kills every later fork in the boot (x86-r3 chunk c8: 91 forkfail
/// cascade).
pub(crate) struct TmpfsPage {
    data: Box<[u8; PAGE_SIZE]>,
    sb: *const TmpfsSuperBlock,
}

impl TmpfsPage {
    fn new(sb: *const TmpfsSuperBlock) -> Self {
        // The caller must have reserved budget (used_pages already
        // incremented) — see TmpfsNode::write_at.
        Self {
            data: Box::new([0u8; PAGE_SIZE]),
            sb,
        }
    }
}

impl Drop for TmpfsPage {
    fn drop(&mut self) {
        if !self.sb.is_null() {
            unsafe {
                (*self.sb).used_pages.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }
}

// SAFETY: the raw superblock pointer is a leaked mount-lifetime
// allocation (Box::into_raw in create_mount_instance); Drop only
// touches an atomic through it.
unsafe impl Send for TmpfsPage {}

// ============================================================================
// Node
// ============================================================================

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum TmpfsType {
    Directory,
    RegularFile,
    SymbolicLink,
}

/// One tmpfs object (file / directory / symlink).
///
/// All mutable state is behind Spinlocks; `name` uses interior mutability
/// under the parent's children lock (identical discipline to RootFSNode).
#[repr(C)]
pub struct TmpfsNode {
    /// Node name (mutated only via set_name with the parent lock held).
    pub(crate) name: UnsafeCell<Vec<u8>>,
    pub node_type: TmpfsType,
    /// Page-granular contents. Shared `Arc` between hard links so writes
    /// through one link are visible through the others (POSIX semantics).
    pages: Arc<Spinlock<BTreeMap<usize, TmpfsPage>>>,
    /// Logical file size in bytes (independent of the allocated page set).
    size: AtomicU64,
    /// File-type + permission bits (chmod-visible).
    pub mode: Spinlock<u32>,
    /// atime / mtime in wall-clock (epoch) seconds.
    atime: AtomicU64,
    mtime: AtomicU64,
    /// Symlink target (None for non-symlinks).
    link_target: Option<Vec<u8>>,
    /// Child nodes (directories only).
    children: Spinlock<Vec<Arc<TmpfsNode>>>,
    /// Inode number within this tmpfs instance.
    pub ino: u64,
    /// Instance identity (icache key low bits are in the node so iget can
    /// stamp VFS inodes without touching the superblock).
    pub fs_id: u64,
    /// Owning superblock (leaked at mount; valid for the kernel's lifetime).
    sb: *const TmpfsSuperBlock,
    /// Device number for device-typed nodes (S_IFCHR/S_IFBLK), USERSPACE
    /// dev_t encoding (new_encode_dev — what mknod(2) passed and stat(2)
    /// reports). 0 for non-device nodes. OH Phase 1b: OH's init mounts a
    /// tmpfs over /dev and mknods /dev/{null,random,urandom,kmsg}; open()
    /// binds those nodes to CharDev registry ops through this number.
    rdev: AtomicU64,
    /// Owner uid/gid, chown-visible. Initialized from the creating task's
    /// fsuid/fsgid (Linux inode_init_owner()); updated by chown/setattr
    /// (LTP creat09 tmpfs round: chown(dir, uid, gid) then stat must
    /// report the new gid).
    uid: AtomicU32,
    gid: AtomicU32,
}

// SAFETY: mutable fields (pages/size/mode/children/atime/mtime) are protected
// by Spinlocks or atomics; name uses UnsafeCell mutated only under the
// parent's children lock.
unsafe impl Send for TmpfsNode {}
unsafe impl Sync for TmpfsNode {}

impl TmpfsNode {
    /// Build a node of the given type with default modes (root-owned).
    fn new(name: Vec<u8>, node_type: TmpfsType, ino: u64, fs_id: u64, sb: *const TmpfsSuperBlock) -> Self {
        let mode = match node_type {
            TmpfsType::Directory => InodeMode::S_IFDIR | 0o777,
            TmpfsType::RegularFile => InodeMode::S_IFREG | 0o666,
            TmpfsType::SymbolicLink => InodeMode::S_IFLNK | 0o777,
        };
        // Linux inode_init_owner(): creator's EFFECTIVE filesystem ids, not
        // the real ones (same rule as ext4_new_inode).
        let (uid, gid) = match crate::sched::current() {
            Some(task) => {
                // SAFETY: sched::current() yields a valid task reference.
                let cred = unsafe { (*task).cred() };
                (cred.fsuid, cred.fsgid)
            }
            None => (0, 0),
        };
        Self {
            name: UnsafeCell::new(name),
            node_type,
            pages: Arc::new(Spinlock::new(BTreeMap::new())),
            size: AtomicU64::new(0),
            mode: Spinlock::new(mode),
            atime: AtomicU64::new(wall_secs()),
            mtime: AtomicU64::new(wall_secs()),
            link_target: None,
            children: Spinlock::new(Vec::new()),
            ino,
            fs_id,
            sb,
            rdev: AtomicU64::new(0),
            uid: AtomicU32::new(uid),
            gid: AtomicU32::new(gid),
        }
    }

    /// Node name as a byte slice.
    pub fn name(&self) -> &[u8] {
        // SAFETY: name is only mutated via set_name(), which requires the
        // parent's children lock (exclusive access).
        unsafe { &*self.name.get() }
    }

    /// Rename in place. Caller must hold the parent's children lock.
    pub fn set_name(&self, new_name: Vec<u8>) {
        // SAFETY: caller guarantees exclusive access (parent lock held).
        unsafe { *self.name.get() = new_name; }
    }

    pub fn is_dir(&self) -> bool {
        self.node_type == TmpfsType::Directory
    }

    pub fn is_file(&self) -> bool {
        self.node_type == TmpfsType::RegularFile
    }

    pub fn is_symlink(&self) -> bool {
        self.node_type == TmpfsType::SymbolicLink
    }

    pub fn add_child(&self, child: Arc<TmpfsNode>) {
        self.children.lock().push(child);
    }

    pub fn remove_child(&self, name: &[u8]) -> bool {
        let mut children = self.children.lock();
        if let Some(pos) = children.iter().position(|c| c.name() == name) {
            children.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn find_child(&self, name: &[u8]) -> Option<Arc<TmpfsNode>> {
        let children = self.children.lock();
        children.iter().find(|c| c.name() == name).cloned()
    }

    pub fn list_children(&self) -> Vec<Arc<TmpfsNode>> {
        self.children.lock().clone()
    }

    pub fn file_size(&self) -> u64 {
        self.size.load(Ordering::Acquire)
    }

    /// Number of allocated pages (stat blocks / memory accounting).
    pub fn page_count(&self) -> u64 {
        self.pages.lock().len() as u64
    }

    /// Read up to `buf.len()` bytes at `offset`. Never-written pages read as
    /// zeros (sparse); reads past EOF return short.
    pub fn read_at(&self, offset: usize, buf: &mut [u8]) -> usize {
        let size = self.size.load(Ordering::Acquire) as usize;
        if offset >= size || buf.is_empty() {
            return 0;
        }
        let avail = size - offset;
        let to_read = buf.len().min(avail);

        let pages = self.pages.lock();
        let mut done = 0usize;
        while done < to_read {
            let off = offset + done;
            let page_idx = off / PAGE_SIZE;
            let page_off = off % PAGE_SIZE;
            let chunk = (to_read - done).min(PAGE_SIZE - page_off);
            if let Some(page) = pages.get(&page_idx) {
                buf[done..done + chunk].copy_from_slice(&page.data[page_off..page_off + chunk]);
            } else {
                // Hole: zero-fill.
                buf[done..done + chunk].fill(0);
            }
            done += chunk;
        }
        drop(pages);

        self.atime.store(wall_secs(), Ordering::Release);
        done
    }

    /// Write `data` at `offset`, allocating pages on demand. Extends the
    /// logical size to cover the stored bytes.
    ///
    /// Returns the number of bytes stored. When the mount's page budget
    /// (mount option `size=`/`nr_blocks=`) runs out mid-write, the stored
    /// prefix is returned — a SHORT count; 0 when not even one new page
    /// could be allocated. The file-op caller maps a zero return to
    /// ENOSPC (POSIX: short write when some bytes fit, ENOSPC when none).
    pub fn write_at(&self, offset: usize, data: &[u8]) -> usize {
        if data.is_empty() {
            return 0;
        }

        let mut done = 0usize;
        {
            let mut pages = self.pages.lock();
            while done < data.len() {
                let off = offset + done;
                let page_idx = off / PAGE_SIZE;
                let page_off = off % PAGE_SIZE;
                let chunk = (data.len() - done).min(PAGE_SIZE - page_off);
                // Allocate the page on first touch (demand paging), under
                // the mount's page budget. reserve_page() consumes one
                // budget slot exactly when the entry is about to be
                // inserted; on failure nothing was reserved (loop breaks).
                if !pages.contains_key(&page_idx) && !self.reserve_page() {
                    break; // budget exhausted: short write (0 => ENOSPC)
                }
                let page = pages
                    .entry(page_idx)
                    .or_insert_with(|| TmpfsPage::new(self.sb));
                page.data[page_off..page_off + chunk]
                    .copy_from_slice(&data[done..done + chunk]);
                done += chunk;
            }
        }

        // size = max(size, offset + done) — only bytes that actually
        // landed extend the logical size (no fetch_max on AtomicU64,
        // CAS loop).
        let end = (offset + done) as u64;
        let mut cur = self.size.load(Ordering::Acquire);
        while end > cur {
            match self
                .size
                .compare_exchange_weak(cur, end, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(actual) => cur = actual,
            }
        }

        self.mtime.store(wall_secs(), Ordering::Release);
        done
    }

    /// Truncate to `new_size`: drop whole pages beyond the new size and
    /// zero the tail of the (kept) partial last page so a later re-extend
    /// exposes zeros, not stale data. Dropping a `TmpfsPage` returns it
    /// to the mount's page budget automatically.
    pub fn truncate(&self, new_size: usize) {
        let mut pages = self.pages.lock();
        if new_size == 0 {
            // A 0-size file keeps NO pages. The old `idx <= new_size /
            // PAGE_SIZE` retained page 0 for new_size == 0 — a leaked page
            // (and, with the size= budget, a leaked budget charge that
            // never returned; caught by the tmpfs-budget unit test).
            pages.clear();
        } else {
            let last_kept_page = new_size / PAGE_SIZE;
            let tail = new_size % PAGE_SIZE;
            pages.retain(|&idx, _| idx <= last_kept_page);
            if tail != 0 {
                if let Some(page) = pages.get_mut(&last_kept_page) {
                    page.data[tail..PAGE_SIZE].fill(0);
                }
            }
        }
        drop(pages);
        self.size.store(new_size as u64, Ordering::Release);
        self.mtime.store(wall_secs(), Ordering::Release);
    }

    /// Reserve one page of the mount's `size=` budget (used_pages +1).
    ///
    /// CAS against `used_pages` so two concurrent writers cannot both
    /// squeeze past the limit. Returns false when the budget is full —
    /// the caller must not allocate. Unlimited mounts (page_limit 0)
    /// always succeed and still count usage (statfs/df later).
    fn reserve_page(&self) -> bool {
        if self.sb.is_null() {
            // Root node before stamp_final_address: directories never
            // take data pages, and the root is always a directory.
            return true;
        }
        unsafe {
            let sb = &*self.sb;
            let limit = sb.page_limit_pages.load(Ordering::Acquire);
            if limit == 0 {
                sb.used_pages.fetch_add(1, Ordering::AcqRel);
                return true;
            }
            let mut cur = sb.used_pages.load(Ordering::Acquire);
            loop {
                if cur >= limit {
                    return false;
                }
                match sb
                    .used_pages
                    .compare_exchange_weak(cur, cur + 1, Ordering::AcqRel, Ordering::Acquire)
                {
                    Ok(_) => return true,
                    Err(actual) => cur = actual,
                }
            }
        }
    }
}

// ============================================================================
// Superblock & instances
// ============================================================================

pub struct TmpfsSuperBlock {
    pub sb: SuperBlock,
    pub root_node: Arc<TmpfsNode>,
    next_ino: AtomicU64,
    /// Instance id (icache key low bits).
    fs_id: u64,
    /// Page budget from the mount option string (`size=`/`nr_blocks=`),
    /// in 4 KiB pages. 0 = unlimited (Linux's default is half of RAM,
    /// which for this heap-backed tmpfs means "bounded by the kernel
    /// heap" — the pre-fix behavior).
    page_limit_pages: AtomicU64,
    /// Live data pages across ALL files of this mount. Incremented by
    /// TmpfsNode::reserve_page, decremented by TmpfsPage::drop.
    used_pages: AtomicU64,
}

impl TmpfsSuperBlock {
    fn new(instance: u64, page_limit_pages: u64) -> Self {
        let fs_id = FS_ID_TMPFS_BASE | instance;
        let root_node = Arc::new(TmpfsNode::new(
            b"/".to_vec(),
            TmpfsType::Directory,
            1,
            fs_id,
            core::ptr::null(), // patched by stamp_final_address() once boxed
        ));
        let mut sb = SuperBlock::new(PAGE_SIZE as usize, TMPFS_MAGIC);
        sb.set_flags(SuperBlockFlags::new(SuperBlockFlags::SB_ACTIVE));

        Self {
            sb,
            root_node,
            next_ino: AtomicU64::new(2),
            fs_id,
            page_limit_pages: AtomicU64::new(page_limit_pages),
            used_pages: AtomicU64::new(0),
        }
    }

    /// Stamp the root node's back-pointer with the superblock's FINAL
    /// address. Must be called after the value is placed at its permanent
    /// home (Box) — a pointer taken to `self` before the move would be a
    /// dangling stack address. No child nodes exist yet at mount time, so
    /// only the root needs stamping (children get the correct `sb` from
    /// `new_node`, which is only reachable through the final address).
    ///
    /// # Safety (caller obligations)
    /// `self_ptr` must point at this superblock's final storage and the
    /// tree must not yet be published to other threads.
    unsafe fn stamp_final_address(self_ptr: *mut TmpfsSuperBlock) {
        // SAFETY: caller guarantees self_ptr is the final, valid address.
        let root = Arc::as_ptr(&(*self_ptr).root_node) as *mut TmpfsNode;
        // SAFETY: the tree is unpublished; the root node is uniquely
        // reachable from this thread.
        (*root).sb = self_ptr as *const TmpfsSuperBlock;
    }

    fn alloc_ino(&self) -> u64 {
        self.next_ino.fetch_add(1, Ordering::AcqRel)
    }

    /// Create a fresh node stamped with this instance's identity.
    fn new_node(&self, name: Vec<u8>, node_type: TmpfsType) -> Arc<TmpfsNode> {
        Arc::new(TmpfsNode::new(
            name,
            node_type,
            self.alloc_ino(),
            self.fs_id,
            self as *const TmpfsSuperBlock,
        ))
    }
}

/// Instance counter + registry of leaked superblocks (mount-lifetime).
static TMPFS_INSTANCE_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Parse the tmpfs page budget out of a mount(2) options string.
///
/// Recognized (Linux tmpfs option table): `size=<bytes>` and the legacy
/// alias `nr_blocks=<pages>`, each with optional k/K, m/M, g/G suffixes
/// (powers of 1024, case-insensitive — mount(8) normalizes to lower
/// case, tmpfs.kopt accepts both). `size` is rounded UP to whole pages,
/// like Linux. A percentage (`size=50%`) maps to the kernel heap's page
/// count (Linux uses totalram; the heap is what tmpfs draws from here).
/// Everything else in the string (mode=, nr_inodes=, mpol=...) is
/// accepted and ignored, like tmpfs does for options it does not
/// implement. Returns 0 when no size option is present (unlimited —
/// Linux's default is RAM/2, i.e. effectively unlimited for a heap of
/// the same box's RAM).
pub(crate) fn parse_size_pages(data: Option<&str>) -> u64 {
    let data = match data {
        Some(d) if !d.is_empty() => d,
        _ => return 0,
    };
    for token in data.split(',') {
        let token = token.trim();
        let (key, value) = match token.split_once('=') {
            Some(kv) => kv,
            None => continue,
        };
        let key = key.trim().to_ascii_lowercase();
        if key != "size" && key != "nr_blocks" {
            continue;
        }
        let value = value.trim();
        if value.is_empty() {
            continue;
        }
        let (digits, percent) = match value.strip_suffix('%') {
            Some(d) => (d, true),
            None => (value, false),
        };
        let digits = digits.trim();
        let (num_str, mult) = match digits.as_bytes().last() {
            Some(b'k') | Some(b'K') => (&digits[..digits.len() - 1], 1024u64),
            Some(b'm') | Some(b'M') => (&digits[..digits.len() - 1], 1024 * 1024),
            Some(b'g') | Some(b'G') => (&digits[..digits.len() - 1], 1024 * 1024 * 1024),
            _ => (digits, 1),
        };
        let num: u64 = match num_str.trim().parse() {
            Ok(n) => n,
            Err(_) => continue, // not a number: ignore the option
        };
        return if percent {
            // percentage of the kernel heap (page count) — Linux maps a
            // percentage onto totalram; the heap is what tmpfs draws
            // from here.
            let heap_pages = (crate::config::KERNEL_HEAP_SIZE / PAGE_SIZE) as u64;
            num.min(100) * heap_pages / 100
        } else if key == "nr_blocks" {
            // nr_blocks counts PAGES (the pre-size= ABI), memparse
            // suffix included.
            num.saturating_mul(mult)
        } else {
            // size= counts bytes; round UP to whole pages like Linux.
            num.saturating_mul(mult).div_ceil(PAGE_SIZE as u64)
        };
    }
    0
}

/// Wrapper so the raw-pointer registry is `Send` (access only under the
/// Spinlock; pointers are leaked mount-lifetime allocations, never freed).
struct TmpfsInstances {
    list: Vec<*const TmpfsSuperBlock>,
}
// SAFETY: the raw pointers are leaked superblocks (mount lifetime); they are
// only copied out and dereferenced under the registry lock.
unsafe impl Send for TmpfsInstances {}

static TMPFS_INSTANCES: Spinlock<TmpfsInstances> =
    Spinlock::new(TmpfsInstances { list: Vec::new() });

/// Create a new independent tmpfs instance and return its root VFS inode.
/// Called by `mount::do_mount("tmpfs")`. `data` is the mount(2) options
/// string ("size=32M,mode=700,..."); None = defaults (unlimited).
pub fn create_mount_instance(data: Option<&str>) -> Arc<Inode> {
    let instance = TMPFS_INSTANCE_COUNTER.fetch_add(1, Ordering::AcqRel);
    let sb = Box::new(TmpfsSuperBlock::new(instance, parse_size_pages(data)));
    let sb_ptr = Box::into_raw(sb) as *mut TmpfsSuperBlock; // leaked: mount lifetime
    // Stamp the root's sb back-pointer with the FINAL (heap) address.
    // SAFETY: sb_ptr is this superblock's final storage; the tree is not
    // yet published (no dentry/inode references exist yet).
    unsafe { TmpfsSuperBlock::stamp_final_address(sb_ptr); }
    TMPFS_INSTANCES.lock_irqsave().list.push(sb_ptr as *const TmpfsSuperBlock);

    // SAFETY: sb_ptr is a leaked, valid superblock.
    let root_node = unsafe { (*sb_ptr).root_node.clone() };
    tmpfs_make_inode(&root_node)
}

/// Build the VFS inode view of a tmpfs node (shared by mount + iget).
fn tmpfs_make_inode(node: &Arc<TmpfsNode>) -> Arc<Inode> {
    let mut inode = Inode::new(node.ino, InodeMode::new(*node.mode.lock()));
    inode.fs_id = node.fs_id;
    inode.size.store(node.file_size(), Ordering::Release);
    inode.uid.store(node.uid.load(Ordering::Relaxed), Ordering::Relaxed);
    inode.gid.store(node.gid.load(Ordering::Relaxed), Ordering::Relaxed);
    // Stamp the device number so mmap(2) can identify /dev/zero-style
    // nodes through the inode's rdev without a getattr round trip (the
    // devfs discipline — see devfs_iget).
    inode.rdev = node.rdev.load(Ordering::Relaxed);
    inode.private_data = Some(Arc::into_raw(Arc::clone(node)) as *mut u8);
    inode.ops = Some(&TMPFS_INODE_OPS);
    Arc::new(inode)
}

// ============================================================================
// Mount / filesystem type registration
// ============================================================================

/// tmpfs mount callback (FsRegistry path). The registry path never wires a
/// dentry tree — it only validates the type name — so a plain SuperBlock is
/// returned. Real instances (with their trees) come from
/// `create_mount_instance()` via `mount::do_mount`.
// SAFETY: FsContext is a valid reference from the registry mount path.
unsafe extern "C" fn tmpfs_mount(_fc: &FsContext) -> Result<*mut SuperBlock, i32> {
    Ok(Box::into_raw(Box::new(SuperBlock::new(PAGE_SIZE, TMPFS_MAGIC))))
}

pub static TMPFS_FS_TYPE: FileSystemType = FileSystemType::new(
    "tmpfs",
    Some(tmpfs_mount),
    None,
    0,
);

static TMPFS_REGISTERED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// Register the "tmpfs" filesystem type in the FsRegistry (idempotent).
/// Needed so the registry-based mount path (superblock::do_mount) accepts
/// the type name; sys_mount dispatches by string via mount::do_mount.
pub fn ensure_registered() {
    if !TMPFS_REGISTERED.swap(true, Ordering::AcqRel) {
        let _ = crate::fs::superblock::register_filesystem(&TMPFS_FS_TYPE);
    }
}

// ============================================================================
// Inode operations
// ============================================================================

/// Recover the node from a VFS inode's private_data.
// SAFETY: private_data was installed by tmpfs_make_inode / tmpfs_iget as an
// Arc::into_raw reference; it stays alive at least as long as the inode.
unsafe fn node_of(inode: &Inode) -> Result<&'static TmpfsNode, i32> {
    match inode.private_data {
        Some(ptr) => Ok(&*(ptr as *const TmpfsNode)),
        None => Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
    }
}

/// Count directory entries referencing `ino` (st_nlink for hard links);
/// directories report 2 + subdirectory count (POSIX). Walks the owning
/// instance's tree — tmpfs trees are small (shm segments, container /tmp).
fn tmpfs_count_links(root: &TmpfsNode, ino: u64) -> u32 {
    fn walk(node: &TmpfsNode, ino: u64, count: &mut u32) {
        if node.is_dir() {
            for child in node.list_children() {
                walk(&child, ino, count);
            }
        } else if node.ino == ino {
            *count += 1;
        }
    }
    let mut count = 0u32;
    walk(root, ino, &mut count);
    count.max(1)
}

/// Reject renaming a directory into its own subtree (cycle guard).
fn tmpfs_subtree_contains(node: &TmpfsNode, candidate: &TmpfsNode) -> bool {
    if core::ptr::eq(node, candidate) {
        return true;
    }
    if node.is_dir() {
        for child in node.list_children() {
            if tmpfs_subtree_contains(&child, candidate) {
                return true;
            }
        }
    }
    false
}

// SAFETY: VFS callback contract — pointers are valid for the call.
unsafe fn tmpfs_lookup(dir: &Inode, name: &[u8]) -> Result<Ino, errno::Errno> {
    let node = node_of(dir)?;
    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory);
    }
    match node.find_child(name) {
        Some(child) => Ok(child.ino),
        None => Err(errno::Errno::NoSuchFileOrDirectory),
    }
}

// SAFETY: VFS callback contract — pointers are valid for the call.
unsafe fn tmpfs_create(dir: &Inode, name: &[u8], mode: InodeMode) -> Result<Arc<Inode>, errno::Errno> {
    let node = node_of(dir)?;
    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory);
    }
    if node.find_child(name).is_some() {
        return Err(errno::Errno::FileExists);
    }
    let sb = node.sb as *const TmpfsSuperBlock;
    // SAFETY: sb is a leaked mount-lifetime superblock.
    let sb = unsafe { &*sb };

    let file = sb.new_node(name.to_vec(), TmpfsType::RegularFile);
    // Honor the caller's permission bits (umask is applied by the VFS layer).
    *file.mode.lock() = InodeMode::S_IFREG | (mode.bits() & 0o7777);
    node.add_child(file.clone());
    Ok(tmpfs_make_inode(&file))
}

// SAFETY: VFS callback contract — pointers are valid for the call.
unsafe fn tmpfs_mkdir(dir: &Inode, name: &[u8], mode: InodeMode) -> Result<Arc<Inode>, errno::Errno> {
    let node = node_of(dir)?;
    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory);
    }
    if node.find_child(name).is_some() {
        return Err(errno::Errno::FileExists);
    }
    let sb = node.sb as *const TmpfsSuperBlock;
    // SAFETY: sb is a leaked mount-lifetime superblock.
    let sb = unsafe { &*sb };

    let d = sb.new_node(name.to_vec(), TmpfsType::Directory);
    *d.mode.lock() = InodeMode::S_IFDIR | (mode.bits() & 0o7777);
    node.add_child(d.clone());
    Ok(tmpfs_make_inode(&d))
}

// SAFETY: VFS callback contract — pointers are valid for the call.
unsafe fn tmpfs_symlink(dir: &Inode, name: &[u8], target: &[u8]) -> Result<Arc<Inode>, errno::Errno> {
    let node = node_of(dir)?;
    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory);
    }
    if node.find_child(name).is_some() {
        return Err(errno::Errno::FileExists);
    }
    let sb = node.sb as *const TmpfsSuperBlock;
    // SAFETY: sb is a leaked mount-lifetime superblock.
    let sb = unsafe { &*sb };

    let link = sb.new_node(name.to_vec(), TmpfsType::SymbolicLink);
    // SAFETY: link is exclusively owned here (not yet in the tree).
    unsafe {
        let link_mut = Arc::as_ptr(&link) as *mut TmpfsNode;
        (*link_mut).link_target = Some(target.to_vec());
    }
    node.add_child(link.clone());
    Ok(tmpfs_make_inode(&link))
}

// SAFETY: VFS callback contract — pointers are valid for the call.
unsafe fn tmpfs_mknod(
    dir: &Inode,
    name: &[u8],
    mode: InodeMode,
    dev: u64,
) -> Result<Arc<Inode>, errno::Errno> {
    let node = node_of(dir)?;
    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory);
    }
    if node.find_child(name).is_some() {
        return Err(errno::Errno::FileExists);
    }

    // may_mknod (Linux): S_IFDIR → EPERM, S_IFLNK/unknown → EINVAL; a
    // zero type field translates to S_IFREG (do_mknodat).
    // (The syscall layer already rejects those for the syscall path; the
    // op-level check keeps direct VFS callers honest.)
    let ftype = mode.bits() & InodeMode::S_IFMT;
    match ftype {
        InodeMode::S_IFCHR | InodeMode::S_IFBLK | InodeMode::S_IFIFO
        | InodeMode::S_IFSOCK | InodeMode::S_IFREG | 0 => {}
        InodeMode::S_IFDIR => return Err(errno::Errno::OperationNotPermitted),
        _ => return Err(errno::Errno::InvalidArgument),
    }
    let ftype = if ftype == 0 { InodeMode::S_IFREG } else { ftype };

    let sb = node.sb as *const TmpfsSuperBlock;
    // SAFETY: sb is a leaked mount-lifetime superblock.
    let sb = unsafe { &*sb };

    // Device nodes are stored as RegularFile nodes whose MODE word carries
    // the type (the same representation the ATTR_MODE retype path uses for
    // mkfifo'd FIFOs) plus the rdev in the node. Reads/writes never touch
    // them: open() binds through the CharDev registry (get_file_ops).
    let file = sb.new_node(name.to_vec(), TmpfsType::RegularFile);
    *file.mode.lock() = ftype | (mode.bits() & 0o7777);
    file.rdev.store(dev, Ordering::Release);
    node.add_child(file.clone());
    Ok(tmpfs_make_inode(&file))
}

// SAFETY: VFS callback contract — pointers are valid for the call.
unsafe fn tmpfs_link(dir: &Inode, name: &[u8], target: &Inode) -> i32 {
    let dir_node = match node_of(dir) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let target_node = match node_of(target) {
        Ok(n) => n,
        Err(e) => return e,
    };
    if !dir_node.is_dir() {
        return errno::Errno::NotADirectory.as_neg_i32();
    }
    if target_node.is_dir() {
        return errno::Errno::IsADirectory.as_neg_i32();
    }
    if dir_node.find_child(name).is_some() {
        return errno::Errno::FileExists.as_neg_i32();
    }

    // Hard link: a new directory entry with the SAME ino and the SHARED
    // page map (POSIX visibility semantics; pages Arc cloned, not copied).
    let mut new_node = TmpfsNode::new(
        name.to_vec(),
        target_node.node_type,
        target_node.ino,
        target_node.fs_id,
        target_node.sb,
    );
    new_node.pages = target_node.pages.clone();
    new_node.size.store(target_node.file_size(), Ordering::Release);
    *new_node.mode.lock() = *target_node.mode.lock();
    new_node.rdev.store(target_node.rdev.load(Ordering::Acquire), Ordering::Release);
    // SAFETY: link_target is set before publication (add_child).
    let new_link = {
        let p = &mut new_node as *mut TmpfsNode;
        unsafe { (*p).link_target = target_node.link_target.clone(); }
        Arc::new(new_node)
    };
    dir_node.add_child(new_link);
    0
}

// SAFETY: VFS callback contract — pointers are valid for the call.
unsafe fn tmpfs_unlink(dir: &Inode, name: &[u8]) -> i32 {
    let node = match node_of(dir) {
        Ok(n) => n,
        Err(e) => return e,
    };
    if !node.is_dir() {
        return errno::Errno::NotADirectory.as_neg_i32();
    }
    match node.find_child(name) {
        Some(child) => {
            if child.is_dir() {
                return errno::Errno::IsADirectory.as_neg_i32();
            }
        }
        None => return errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    }
    if node.remove_child(name) {
        0
    } else {
        errno::Errno::NoSuchFileOrDirectory.as_neg_i32()
    }
}

// SAFETY: VFS callback contract — pointers are valid for the call.
unsafe fn tmpfs_rmdir(dir: &Inode, name: &[u8]) -> i32 {
    let node = match node_of(dir) {
        Ok(n) => n,
        Err(e) => return e,
    };
    if !node.is_dir() {
        return errno::Errno::NotADirectory.as_neg_i32();
    }
    match node.find_child(name) {
        Some(child) => {
            if !child.is_dir() {
                return errno::Errno::NotADirectory.as_neg_i32();
            }
            if !child.list_children().is_empty() {
                return errno::Errno::DirectoryNotEmpty.as_neg_i32();
            }
        }
        None => return errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    }
    if node.remove_child(name) {
        0
    } else {
        errno::Errno::NoSuchFileOrDirectory.as_neg_i32()
    }
}

// SAFETY: VFS callback contract — pointers are valid for the call.
unsafe fn tmpfs_rename(old_dir: &Inode, old_name: &[u8], new_dir: &Inode, new_name: &[u8]) -> i32 {
    let old_dir_node = match node_of(old_dir) {
        Ok(n) => n,
        Err(e) => return e,
    };
    let new_dir_node = match node_of(new_dir) {
        Ok(n) => n,
        Err(e) => return e,
    };
    if !old_dir_node.is_dir() || !new_dir_node.is_dir() {
        return errno::Errno::NotADirectory.as_neg_i32();
    }

    let source = match old_dir_node.find_child(old_name) {
        Some(n) => n,
        None => return errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    };

    // Cycle guard: destination directory must not live inside source's
    // subtree (renaming a dir into its own child orphans it).
    if source.is_dir() && tmpfs_subtree_contains(&source, new_dir_node) {
        return errno::Errno::InvalidArgument.as_neg_i32();
    }

    // POSIX rename overwrites an existing destination (unless it is a
    // non-empty directory).
    if let Some(existing) = new_dir_node.find_child(new_name) {
        if existing.is_dir() {
            if !existing.list_children().is_empty() {
                return errno::Errno::DirectoryNotEmpty.as_neg_i32();
            }
        }
        new_dir_node.remove_child(new_name);
    }

    // Remove under the OLD name first (remove_child matches by name), then
    // rename, then add — source is exclusively held between the two.
    if !old_dir_node.remove_child(old_name) {
        return errno::Errno::NoSuchFileOrDirectory.as_neg_i32();
    }
    source.set_name(new_name.to_vec());
    new_dir_node.add_child(source);
    0
}

// SAFETY: VFS callback contract — pointers are valid for the call.
unsafe fn tmpfs_readlink(inode: &Inode, buf: &mut [u8]) -> isize {
    let node = match node_of(inode) {
        Ok(n) => n,
        Err(e) => return e as isize,
    };
    if !node.is_symlink() {
        return errno::Errno::InvalidArgument.as_neg_i32() as isize;
    }
    match &node.link_target {
        Some(target) => {
            let len = target.len().min(buf.len());
            buf[..len].copy_from_slice(&target[..len]);
            len as isize
        }
        None => errno::Errno::IOError.as_neg_i32() as isize,
    }
}

// SAFETY: VFS callback contract — pointers are valid for the call.
unsafe fn tmpfs_getattr(inode: &Inode, stat: &mut crate::fs::Stat) -> i32 {
    let node = match node_of(inode) {
        Ok(n) => n,
        Err(e) => return e,
    };
    stat.st_ino = node.ino;
    stat.st_mode = *node.mode.lock();
    stat.st_size = node.file_size() as i64;
    stat.st_nlink = if node.is_dir() {
        // 2 ('.' + parent entry) + one per subdirectory.
        2 + node.list_children().iter().filter(|c| c.is_dir()).count() as u32
    } else {
        // Hard links share the ino; count referencing entries.
        let sb = node.sb as *const TmpfsSuperBlock;
        // SAFETY: sb is a leaked mount-lifetime superblock.
        unsafe { tmpfs_count_links(&(*sb).root_node, node.ino) }
    };
    stat.st_uid = node.uid.load(Ordering::Relaxed) as u32;
    stat.st_gid = node.gid.load(Ordering::Relaxed) as u32;
    // st_rdev: the userspace dev_t of mknod-created device nodes (0 for
    // everything else) — major()/minor() decode in userspace.
    stat.st_rdev = node.rdev.load(Ordering::Relaxed);
    stat.st_blksize = PAGE_SIZE as i64;
    stat.st_blocks = (node.page_count() * (PAGE_SIZE as u64 / 512)) as i64;
    stat.st_atime = node.atime.load(Ordering::Relaxed) as i64;
    stat.st_atime_nsec = 0;
    stat.st_mtime = node.mtime.load(Ordering::Relaxed) as i64;
    stat.st_mtime_nsec = 0;
    stat.st_ctime = node.mtime.load(Ordering::Relaxed) as i64;
    stat.st_ctime_nsec = 0;
    0
}

// SAFETY: VFS callback contract — pointers are valid for the call.
unsafe fn tmpfs_setattr(inode: &Inode, attr: u32, arg1: u64, arg2: u64) -> i32 {
    let node = match node_of(inode) {
        Ok(n) => n,
        Err(e) => return e,
    };
    match attr {
        setattr_attr::ATTR_SIZE => {
            node.truncate(arg1 as usize);
            inode.size.store(arg1, Ordering::Release);
            0
        }
        setattr_attr::ATTR_MODE => {
            // Honor type bits in `value` — AF_UNIX bind(2) and mkfifo(3)
            // create their special nodes by create-regular-then-retype
            // (the mknod pattern). A perm-only word (chmod) preserves the
            // current file type from the stored mode word, like Linux.
            let v = arg1 as u32;
            let vtype = v & InodeMode::S_IFMT;
            let file_type = if vtype != 0 {
                vtype
            } else {
                *node.mode.lock() & InodeMode::S_IFMT
            };
            let new_mode = file_type | (v & 0o7777);
            *node.mode.lock() = new_mode;
            // Icache coherency: open()/DAC read the cached Inode.mode
            // while stat() reads through getattr (node.mode) — keep both
            // in sync or retyped FIFOs stay "regular" for open.
            inode.update_cached_mode(InodeMode::new(new_mode));
            node.mtime.store(wall_secs(), Ordering::Release);
            0
        }
        setattr_attr::ATTR_ATIME => {
            node.atime.store(arg1, Ordering::Release);
            0
        }
        setattr_attr::ATTR_MTIME => {
            node.mtime.store(arg1, Ordering::Release);
            0
        }
        setattr_attr::ATTR_UID_GID => {
            // arg1 = uid, arg2 = gid (same encoding as ext4_setattr; the
            // chown syscall and the VFS setgid-directory gid inheritance
            // both use it). u32::MAX means "leave unchanged" (chown(2)
            // with -1). Keep the VFS-cached owner in sync — DAC and owner
            // checks read Inode.uid/gid while stat() goes through getattr.
            let old_uid = node.uid.load(Ordering::Relaxed);
            let old_gid = node.gid.load(Ordering::Relaxed);
            let new_uid = if arg1 == u32::MAX as u64 {
                old_uid
            } else {
                arg1 as u32
            };
            let new_gid = if arg2 == u32::MAX as u64 {
                old_gid
            } else {
                arg2 as u32
            };
            node.uid.store(new_uid, Ordering::Relaxed);
            node.gid.store(new_gid, Ordering::Relaxed);
            inode.uid.store(new_uid, Ordering::Relaxed);
            inode.gid.store(new_gid, Ordering::Relaxed);
            node.mtime.store(wall_secs(), Ordering::Release);
            0
        }
        _ => -95, // EOPNOTSUPP (matches rootfs_setattr's fallback)
    }
}

// SAFETY: VFS callback contract — pointers are valid for the call.
unsafe fn tmpfs_readdir(inode: &Inode) -> Option<Vec<VfsDirEntry>> {
    let node = node_of(inode).ok()?;
    if !node.is_dir() {
        return None;
    }
    let mut entries = Vec::new();
    for child in node.list_children() {
        // d_type from the MODE word, not the node type: FIFOs and unix
        // sockets are created as RegularFile nodes and retyped through
        // chmod-style ATTR_MODE (mkfifo / AF_UNIX bind). Deriving from
        // node_type would hide them as DT_REG — readdir on /run must show
        // bound sockets (DT_SOCK) and named pipes (DT_FIFO).
        let mode = *child.mode.lock();
        let dt = match mode & InodeMode::S_IFMT {
            InodeMode::S_IFDIR => file_type::DT_DIR,
            InodeMode::S_IFLNK => file_type::DT_LNK,
            InodeMode::S_IFIFO => file_type::DT_FIFO,
            InodeMode::S_IFSOCK => file_type::DT_SOCK,
            InodeMode::S_IFCHR => file_type::DT_CHR,
            InodeMode::S_IFBLK => file_type::DT_BLK,
            _ => {
                if child.is_dir() {
                    file_type::DT_DIR
                } else if child.is_symlink() {
                    file_type::DT_LNK
                } else {
                    file_type::DT_REG
                }
            }
        };
        entries.push(VfsDirEntry {
            ino: child.ino,
            name: child.name().to_vec(),
            file_type: dt,
        });
    }
    Some(entries)
}

// SAFETY: VFS callback contract — pointers are valid for the call.
unsafe fn tmpfs_iget(parent: &Inode, name: &[u8], ino: Ino) -> Result<Arc<Inode>, errno::Errno> {
    let node = node_of(parent)?;
    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory);
    }
    let child = node
        .find_child(name)
        .filter(|c| c.ino == ino)
        .ok_or_else(|| errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;
    Ok(tmpfs_make_inode(&child))
}

// SAFETY: VFS callback contract — called when the inode's refcount hits 0.
unsafe fn tmpfs_destroy_inode(inode: &mut Inode) {
    if let Some(ptr) = inode.private_data.take() {
        // Reclaim the leaked Arc reference (idempotent via Option::take).
        let _ = Arc::from_raw(ptr as *const TmpfsNode);
    }
}

// ============================================================================
// File operations (regular files)
// ============================================================================

/// tmpfs regular-file read: page-granular, zero-fill holes, update f_pos.
fn tmpfs_file_read(file: &crate::fs::File, buf: &mut [u8]) -> isize {
    // SAFETY: inode/private_data installed by tmpfs_make_inode; the File
    // holds an Arc<Inode> keeping the node reference alive.
    unsafe {
        let inode = match (&*file.inode.get()).as_ref() {
            Some(i) => i,
            None => return -9,
        };
        let node = match node_of(inode) {
            Ok(n) => n,
            Err(_) => return -9,
        };
        // Same pos-race serialization as reg_file_read: forked children
        // sharing this open file description must not lose offset updates
        // (LTP fork07 — see File::io_lock).
        let _io_guard = file.io_lock.guard();
        let offset = file.get_pos() as usize;
        let n = node.read_at(offset, buf);
        if n > 0 {
            file.set_pos((offset + n) as u64);
        }
        // TEST-ONLY (gdmr24 dbtrace, removed before merge): udev db
        // files under /run/udev/data read back with "G:\n" spliced
        // before each tag line — sd-device parse aborts the tag
        // (EINVAL), so card0 loses master-of-seat in enumeration.
        // Trace db-shaped reads: first byte 'I'/'G'/'Q'/'S'/'E'/'V'
        // followed by ':' narrows to db lines.
        if n > 0
            && buf.len() > 1
            && matches!(buf[0], b'I' | b'G' | b'Q' | b'S' | b'E' | b'V')
            && buf[1] == b':'
            && crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::DbFileTrace)
        {
            let show = n.min(80);
            crate::println!(
                "DBR off={} n={} {:?}..",
                offset,
                n,
                core::str::from_utf8(&buf[..show]).map(|s| s.replace('\n', "\\n")).unwrap_or_else(|_| "<bin>".into())
            );
        }
        n as isize
    }
}

/// tmpfs regular-file write: demand page allocation, O_APPEND honored by
/// the shared reg-file convention (io_lock + end positioning).
///
/// A zero-byte write against a full `size=` budget returns ENOSPC
/// (POSIX: "if some bytes can be written, the write shall return the
/// number ... otherwise -1 with ENOSPC"). Short counts are returned as
/// written and the caller retries — the next call hits ENOSPC.
fn tmpfs_file_write(file: &crate::fs::File, buf: &[u8]) -> isize {
    // SAFETY: see tmpfs_file_read.
    unsafe {
        let inode = match (&*file.inode.get()).as_ref() {
            Some(i) => i,
            None => return -9,
        };
        let node = match node_of(inode) {
            Ok(n) => n,
            Err(_) => return -9,
        };
        let _write_guard = file.io_lock.guard();
        let offset = if file.flags_bits() & crate::fs::FileFlags::O_APPEND != 0 {
            let end = node.file_size();
            file.set_pos(end);
            end as usize
        } else {
            file.get_pos() as usize
        };
        // TEST-ONLY (gdmr24 dbtrace, removed before merge): see the
        // matching hook in tmpfs_file_read.
        if !buf.is_empty()
            && matches!(buf[0], b'I' | b'G' | b'Q' | b'S' | b'E' | b'V')
            && buf.get(1) == Some(&b':')
            && crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::DbFileTrace)
        {
            let show = buf.len().min(80);
            crate::println!(
                "DBW off={} len={} {:?}..",
                offset,
                buf.len(),
                core::str::from_utf8(&buf[..show]).map(|s| s.replace('\n', "\\n")).unwrap_or_else(|_| "<bin>".into())
            );
        }
        let written = node.write_at(offset, buf);
        inode.size.store(node.file_size(), Ordering::Release);
        if written == 0 && !buf.is_empty() {
            // The mount's page budget (size=) is exhausted and this write
            // could not store a single byte. (write_at only returns 0 for
            // an empty buffer otherwise, handled above.)
            return crate::errno::Errno::NoSpaceLeftOnDevice.as_neg_i32() as isize;
        }
        if written > 0 {
            file.set_pos((offset + written) as u64);
        }
        written as isize
    }
}

fn tmpfs_file_lseek(file: &crate::fs::File, offset: isize, whence: i32) -> isize {
    // SAFETY: see tmpfs_file_read.
    let file_size = unsafe {
        match (&*file.inode.get()).as_ref() {
            Some(inode) => match node_of(inode) {
                Ok(node) => node.file_size() as isize,
                Err(_) => return -9,
            },
            None => return -9,
        }
    };
    let current_pos = file.get_pos() as isize;
    let new_pos = match whence {
        0 => offset,
        1 => current_pos + offset,
        2 => file_size + offset,
        _ => return -22,
    };
    if new_pos < 0 {
        return -22;
    }
    file.set_pos(new_pos as u64);
    new_pos
}

fn tmpfs_file_close(_file: &crate::fs::File) -> i32 {
    0
}

/// tmpfs file operations table (directories use the shared DIR_FILE_OPS).
pub static TMPFS_FILE_OPS: crate::fs::FileOps = crate::fs::FileOps::builder()
    .read(tmpfs_file_read)
    .write(tmpfs_file_write)
    .lseek(tmpfs_file_lseek)
    .close(tmpfs_file_close)
    .read_at(tmpfs_file_read_at)
    .build();

/// tmpfs positional read (R71): explicit offset, shared file position
/// untouched — the generic set_pos/read/restore fallback races against
/// concurrent plain reads on the same open file description (demand
/// faults + worker threads), which is how mapped tmpfs pages (dconf's
/// /run/user/*/dc/shm) came back with wrong content.
///
/// SAFETY: inode/private_data installed by tmpfs_make_inode; the File
/// holds an Arc<Inode> keeping the node reference alive.
fn tmpfs_file_read_at(file: &crate::fs::File, offset: u64, buf: &mut [u8]) -> isize {
    unsafe {
        let inode = match (&*file.inode.get()).as_ref() {
            Some(i) => i,
            None => return -9,
        };
        let node = match node_of(inode) {
            Ok(n) => n,
            Err(_) => return -9,
        };
        // Serialize with plain read/write for node-page consistency; the
        // file position is never accessed.
        let _io_guard = file.io_lock.guard();
        let n = node.read_at(offset as usize, buf);
        if n > 0 {
            crate::fs::inotify::notify_file(
                file,
                crate::fs::inotify::inotify_events::IN_ACCESS,
            );
        }
        n as isize
    }
}

// SAFETY: VFS callback contract — pointers are valid for the call.
unsafe fn tmpfs_get_file_ops(inode: &Inode) -> Option<&'static crate::fs::file::FileOps> {
    if inode.mode.is_directory() {
        Some(&crate::fs::file::DIR_FILE_OPS)
    } else if inode.mode.is_regular_file() {
        Some(&TMPFS_FILE_OPS)
    } else if inode.mode.is_char_device() {
        // mknod-created char nodes bind to the CharDev registry through
        // the node's stored rdev (the devfs discipline without the
        // DevfsEntry indirection). No registered driver → ENXIO at open,
        // exactly like devfs.
        let node = node_of(inode).ok()?;
        let rdev = node.rdev.load(Ordering::Relaxed);
        let devno = crate::fs::dev_t::DevNo::from_user_dev(rdev);
        crate::fs::devfs::registry::get_char_device_ops(devno)
    } else {
        // Block devices (and anything else): no block-driver open path —
        // the VFS device-open gate fails with ENXIO, the Linux behavior
        // for a node with no bound driver. FIFOs are dispatched by the
        // open path before get_file_ops; unix sockets by the socket layer.
        None
    }
}

pub static TMPFS_INODE_OPS: INodeOps = INodeOps {
    lookup: Some(tmpfs_lookup),
    create: Some(tmpfs_create),
    link: Some(tmpfs_link),
    unlink: Some(tmpfs_unlink),
    symlink: Some(tmpfs_symlink),
    mkdir: Some(tmpfs_mkdir),
    rmdir: Some(tmpfs_rmdir),
    mknod: Some(tmpfs_mknod),
    rename: Some(tmpfs_rename),
    readlink: Some(tmpfs_readlink),
    get_file_ops: Some(tmpfs_get_file_ops),
    readdir: Some(tmpfs_readdir),
    readdir_cacheable: true,
    open: None,
    permission: None, // default: allow all (DAC via generic path)
    getattr: Some(tmpfs_getattr),
    setattr: Some(tmpfs_setattr),
    iget: Some(tmpfs_iget),
    destroy_inode: Some(tmpfs_destroy_inode),
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tmpfs_page_roundtrip() {
        let sb = TmpfsSuperBlock::new(9001, 0);
        let f = sb.new_node(b"file".to_vec(), TmpfsType::RegularFile);
        // Sparse write: page 0 and page 2, hole at page 1.
        f.write_at(0, b"hello");
        f.write_at(PAGE_SIZE * 2, b"world");
        assert_eq!(f.file_size(), (PAGE_SIZE * 2 + 5) as u64);
        assert_eq!(f.page_count(), 2);

        let mut buf = [0u8; PAGE_SIZE * 3];
        let n = f.read_at(0, &mut buf);
        assert_eq!(n, PAGE_SIZE * 2 + 5);
        assert_eq!(&buf[..5], b"hello");
        // Hole reads as zeros.
        assert!(buf[PAGE_SIZE..PAGE_SIZE * 2].iter().all(|&b| b == 0));
        assert_eq!(&buf[PAGE_SIZE * 2..PAGE_SIZE * 2 + 5], b"world");
    }

    #[test]
    fn test_tmpfs_truncate_zero_tail() {
        let sb = TmpfsSuperBlock::new(9002, 0);
        let f = sb.new_node(b"file".to_vec(), TmpfsType::RegularFile);
        f.write_at(0, &[0xAAu8; 64]);
        f.truncate(16);
        assert_eq!(f.file_size(), 16);
        // Re-extend: the stale tail must read as zeros.
        f.truncate(64);
        let mut buf = [0u8; 64];
        assert_eq!(f.read_at(0, &mut buf), 64);
        assert!(buf[16..].iter().all(|&b| b == 0));
    }

    #[test]
    fn test_tmpfs_dir_ops() {
        let sb = TmpfsSuperBlock::new(9003, 0);
        let root = sb.root_node.clone();
        let d = sb.new_node(b"d".to_vec(), TmpfsType::Directory);
        root.add_child(d.clone());
        assert!(root.find_child(b"d").is_some());
        assert!(d.list_children().is_empty());
        assert!(root.remove_child(b"d"));
        assert!(root.find_child(b"d").is_none());
    }

}

// ============================================================================
// In-kernel unit tests (unit-test feature)
// ============================================================================

/// Boot-time tests for the tmpfs page budget (mount options `size=` /
/// `nr_blocks=`). Lives in this module rather than kernel/src/tests/
/// because it needs private access: `TmpfsSuperBlock::new` with an
/// explicit page limit plus the superblock's live counters. Registered
/// from `crate::tests::run_all_tests()`.
///
/// Covers the LTP preadv203 regression shape: a size-capped tmpfs fed
/// more data than its budget must stop granting pages (short write /
/// ENOSPC), never eat the kernel heap.
#[cfg(feature = "unit-test")]
pub fn run_budget_tests() {
    crate::tests::test_group_start("tmpfs-budget");

    // ---- parse_size_pages: mount option string -> page budget ----------
    crate::test_assert_eq!(parse_size_pages(None), 0, "parse: no options -> unlimited");
    crate::test_assert_eq!(parse_size_pages(Some("")), 0, "parse: empty string -> unlimited");
    crate::test_assert_eq!(parse_size_pages(Some("size=32M")), 8192, "parse: size=32M -> 8192 pages");
    crate::test_assert_eq!(parse_size_pages(Some("size=32m")), 8192, "parse: size=32m (lowercase)");
    crate::test_assert_eq!(parse_size_pages(Some("size=1G")), 256 * 1024, "parse: size=1G");
    crate::test_assert_eq!(parse_size_pages(Some("size=512k")), 128, "parse: size=512k");
    // Round UP to whole pages (Linux memparse + PAGE_ALIGN).
    crate::test_assert_eq!(parse_size_pages(Some("size=1")), 1, "parse: size=1 rounds up to 1 page");
    crate::test_assert_eq!(parse_size_pages(Some("size=4097")), 2, "parse: size=4097 rounds up to 2 pages");
    // nr_blocks counts pages.
    crate::test_assert_eq!(parse_size_pages(Some("nr_blocks=100")), 100, "parse: nr_blocks=100");
    crate::test_assert_eq!(parse_size_pages(Some("nr_blocks=10k")), 10240, "parse: nr_blocks=10k");
    // Mixed with other (ignored) options.
    crate::test_assert_eq!(parse_size_pages(Some("mode=700,size=32M")), 8192, "parse: leading ignored option");
    crate::test_assert_eq!(parse_size_pages(Some("size=32M,nr_inodes=1024")), 8192, "parse: trailing ignored option");
    // Percent of the kernel heap.
    let heap_pages = (crate::config::KERNEL_HEAP_SIZE / PAGE_SIZE) as u64;
    crate::test_assert_eq!(parse_size_pages(Some("size=50%")), heap_pages / 2, "parse: size=50% of heap");
    crate::test_assert_eq!(parse_size_pages(Some("size=200%")), heap_pages, "parse: size=200% clamps to heap");
    // Unknown/garbage values ignored (unlimited).
    crate::test_assert_eq!(parse_size_pages(Some("size=abc")), 0, "parse: garbage value ignored");
    crate::test_assert_eq!(parse_size_pages(Some("nr_inodes=100")), 0, "parse: unrelated option ignored");

    // ---- budget enforcement: short writes, ENOSPC shape -----------------
    // LTP preadv203 pattern: a 32MB-capped tmpfs fed ~118MB of writes
    // must stop granting pages, not eat the kernel heap.
    let sb = TmpfsSuperBlock::new(9004, 2); // 2-page budget
    crate::test_assert_eq!(sb.page_limit_pages.load(Ordering::Relaxed), 2, "budget: limit stored");
    let f = sb.new_node(b"file".to_vec(), TmpfsType::RegularFile);

    // Fill the budget: two distinct pages.
    crate::test_assert_eq!(f.write_at(0, &[1u8; PAGE_SIZE]), PAGE_SIZE, "budget: page 0 stored");
    crate::test_assert_eq!(f.write_at(PAGE_SIZE, &[2u8; PAGE_SIZE]), PAGE_SIZE, "budget: page 1 stored");
    crate::test_assert_eq!(sb.used_pages.load(Ordering::Relaxed), 2, "budget: both pages charged");

    // Third page: refused — short write of the bytes that DID fit
    // (none here: the write starts on a fresh page).
    crate::test_assert_eq!(f.write_at(PAGE_SIZE * 2, &[3u8; PAGE_SIZE]), 0, "budget: third page refused (0 -> ENOSPC)");
    // Partial write: the first page of the range already exists, so
    // its bytes store and the write stops at the page boundary.
    let big = [7u8; PAGE_SIZE * 2];
    let n = f.write_at(PAGE_SIZE, &big);
    crate::test_assert_eq!(n, PAGE_SIZE, "budget: partial write stops at page boundary");
    crate::test_assert_eq!(sb.used_pages.load(Ordering::Relaxed), 2, "budget: still two pages charged");

    // Truncate returns pages to the budget and writes succeed again.
    f.truncate(0);
    crate::test_assert_eq!(sb.used_pages.load(Ordering::Relaxed), 0, "budget: truncate returns pages");
    crate::test_assert_eq!(f.write_at(0, &[9u8; PAGE_SIZE]), PAGE_SIZE, "budget: write works after truncate");

    // Dropping the node returns everything.
    drop(f);
    crate::test_assert_eq!(sb.used_pages.load(Ordering::Relaxed), 0, "budget: drop returns pages");

    // ---- budget is per-MOUNT, shared across files -----------------------
    // Two files on one 1-page tmpfs cannot each take a page.
    let sb = TmpfsSuperBlock::new(9005, 1);
    let a = sb.new_node(b"a".to_vec(), TmpfsType::RegularFile);
    let b = sb.new_node(b"b".to_vec(), TmpfsType::RegularFile);
    crate::test_assert_eq!(a.write_at(0, &[1u8; 16]), 16, "shared: first file takes the page");
    crate::test_assert_eq!(b.write_at(0, &[1u8; 16]), 0, "shared: second file refused (ENOSPC)");
    crate::test_assert_eq!(sb.used_pages.load(Ordering::Relaxed), 1, "shared: one page charged");
    // Hard links share one pages map — no double charge.
    let a2 = a.clone();
    crate::test_assert_eq!(a2.write_at(100, &[5u8; 16]), 16, "shared: hard link writes into same page");
    crate::test_assert_eq!(sb.used_pages.load(Ordering::Relaxed), 1, "shared: no double charge for links");
}
