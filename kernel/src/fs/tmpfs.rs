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
use core::sync::atomic::{AtomicU64, Ordering};
use crate::sync::spinlock::Spinlock;

/// Linux TMPFS_MAGIC.
pub const TMPFS_MAGIC: u32 = 0x0102_1994;

/// tmpfs page size (matches the kernel page size / stat blksize).
const PAGE_SIZE: usize = 4096;

/// icache identity prefix ("TPFS"); the low bits carry the per-mount
/// instance id so (ino, fs_id) pairs from different tmpfs mounts never
/// collide (VFS-H8 discipline — see FS_ID_ROOTFS in inode.rs).
const FS_ID_TMPFS_BASE: u64 = 0x5450_4653_0000_0000;

/// Monotonic boot-clock seconds (timestamp source, same as rootfs).
fn uptime_secs() -> u64 {
    crate::fs::procfs::get_uptime_secs()
}

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
    pages: Arc<Spinlock<BTreeMap<usize, Box<[u8; PAGE_SIZE]>>>>,
    /// Logical file size in bytes (independent of the allocated page set).
    size: AtomicU64,
    /// File-type + permission bits (chmod-visible).
    pub mode: Spinlock<u32>,
    /// atime / mtime in boot-clock seconds.
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
        Self {
            name: UnsafeCell::new(name),
            node_type,
            pages: Arc::new(Spinlock::new(BTreeMap::new())),
            size: AtomicU64::new(0),
            mode: Spinlock::new(mode),
            atime: AtomicU64::new(uptime_secs()),
            mtime: AtomicU64::new(uptime_secs()),
            link_target: None,
            children: Spinlock::new(Vec::new()),
            ino,
            fs_id,
            sb,
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
                buf[done..done + chunk].copy_from_slice(&page[page_off..page_off + chunk]);
            } else {
                // Hole: zero-fill.
                buf[done..done + chunk].fill(0);
            }
            done += chunk;
        }
        drop(pages);

        self.atime.store(uptime_secs(), Ordering::Release);
        done
    }

    /// Write `data` at `offset`, allocating pages on demand. Extends the
    /// logical size to cover the write.
    pub fn write_at(&self, offset: usize, data: &[u8]) -> usize {
        if data.is_empty() {
            return 0;
        }
        let end = offset + data.len();

        {
            let mut pages = self.pages.lock();
            let mut done = 0usize;
            while done < data.len() {
                let off = offset + done;
                let page_idx = off / PAGE_SIZE;
                let page_off = off % PAGE_SIZE;
                let chunk = (data.len() - done).min(PAGE_SIZE - page_off);
                // Allocate the page on first touch (demand paging).
                let page = pages
                    .entry(page_idx)
                    .or_insert_with(|| Box::new([0u8; PAGE_SIZE]));
                page[page_off..page_off + chunk].copy_from_slice(&data[done..done + chunk]);
                done += chunk;
            }
        }

        // size = max(size, end) — no fetch_max on AtomicU64, CAS loop.
        let mut cur = self.size.load(Ordering::Acquire);
        while end as u64 > cur {
            match self
                .size
                .compare_exchange_weak(cur, end as u64, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => break,
                Err(actual) => cur = actual,
            }
        }

        self.mtime.store(uptime_secs(), Ordering::Release);
        data.len()
    }

    /// Truncate to `new_size`: drop whole pages beyond the new size and
    /// zero the tail of the (kept) partial last page so a later re-extend
    /// exposes zeros, not stale data.
    pub fn truncate(&self, new_size: usize) {
        let last_kept_page = new_size / PAGE_SIZE;
        let tail = new_size % PAGE_SIZE;
        let mut pages = self.pages.lock();
        pages.retain(|&idx, _| idx <= last_kept_page);
        if tail != 0 {
            if let Some(page) = pages.get_mut(&last_kept_page) {
                page[tail..PAGE_SIZE].fill(0);
            }
        }
        drop(pages);
        self.size.store(new_size as u64, Ordering::Release);
        self.mtime.store(uptime_secs(), Ordering::Release);
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
}

impl TmpfsSuperBlock {
    fn new(instance: u64) -> Self {
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
/// Called by `mount::do_mount("tmpfs")`.
pub fn create_mount_instance() -> Arc<Inode> {
    let instance = TMPFS_INSTANCE_COUNTER.fetch_add(1, Ordering::AcqRel);
    let sb = Box::new(TmpfsSuperBlock::new(instance));
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
unsafe fn tmpfs_lookup(dir: &Inode, name: &[u8]) -> Result<Ino, i32> {
    let node = node_of(dir)?;
    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory.as_neg_i32());
    }
    match node.find_child(name) {
        Some(child) => Ok(child.ino),
        None => Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
    }
}

// SAFETY: VFS callback contract — pointers are valid for the call.
unsafe fn tmpfs_create(dir: &Inode, name: &[u8], mode: InodeMode) -> Result<Arc<Inode>, i32> {
    let node = node_of(dir)?;
    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory.as_neg_i32());
    }
    if node.find_child(name).is_some() {
        return Err(errno::Errno::FileExists.as_neg_i32());
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
unsafe fn tmpfs_mkdir(dir: &Inode, name: &[u8], mode: InodeMode) -> Result<Arc<Inode>, i32> {
    let node = node_of(dir)?;
    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory.as_neg_i32());
    }
    if node.find_child(name).is_some() {
        return Err(errno::Errno::FileExists.as_neg_i32());
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
unsafe fn tmpfs_symlink(dir: &Inode, name: &[u8], target: &[u8]) -> Result<Arc<Inode>, i32> {
    let node = node_of(dir)?;
    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory.as_neg_i32());
    }
    if node.find_child(name).is_some() {
        return Err(errno::Errno::FileExists.as_neg_i32());
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
    stat.st_uid = 0;
    stat.st_gid = 0;
    stat.st_rdev = 0;
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
unsafe fn tmpfs_setattr(inode: &Inode, attr: u32, value: u64, _value2: u64) -> i32 {
    let node = match node_of(inode) {
        Ok(n) => n,
        Err(e) => return e,
    };
    match attr {
        setattr_attr::ATTR_SIZE => {
            node.truncate(value as usize);
            inode.size.store(value, Ordering::Release);
            0
        }
        setattr_attr::ATTR_MODE => {
            let file_type = match node.node_type {
                TmpfsType::Directory => InodeMode::S_IFDIR,
                TmpfsType::RegularFile => InodeMode::S_IFREG,
                TmpfsType::SymbolicLink => InodeMode::S_IFLNK,
            };
            *node.mode.lock() = file_type | (value as u32 & 0o7777);
            node.mtime.store(uptime_secs(), Ordering::Release);
            0
        }
        setattr_attr::ATTR_ATIME => {
            node.atime.store(value, Ordering::Release);
            0
        }
        setattr_attr::ATTR_MTIME => {
            node.mtime.store(value, Ordering::Release);
            0
        }
        setattr_attr::ATTR_UID_GID => {
            // tmpfs objects are root-owned in this wave; chown accepted,
            // stored nowhere (same policy as rootfs).
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
        let dt = if child.is_dir() {
            file_type::DT_DIR
        } else if child.is_file() {
            file_type::DT_REG
        } else if child.is_symlink() {
            file_type::DT_LNK
        } else {
            file_type::DT_UNKNOWN
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
unsafe fn tmpfs_iget(parent: &Inode, name: &[u8], ino: Ino) -> Result<Arc<Inode>, i32> {
    let node = node_of(parent)?;
    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory.as_neg_i32());
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
        let offset = file.get_pos() as usize;
        let n = node.read_at(offset, buf);
        if n > 0 {
            file.set_pos((offset + n) as u64);
        }
        n as isize
    }
}

/// tmpfs regular-file write: demand page allocation, O_APPEND honored by
/// the shared reg-file convention (write_lock + end positioning).
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
        let _write_guard = file.write_lock.lock();
        let offset = if file.flags_bits() & crate::fs::FileFlags::O_APPEND != 0 {
            let end = node.file_size();
            file.set_pos(end);
            end as usize
        } else {
            file.get_pos() as usize
        };
        let written = node.write_at(offset, buf);
        inode.size.store(node.file_size(), Ordering::Release);
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
pub static TMPFS_FILE_OPS: crate::fs::FileOps = crate::fs::FileOps {
    read: Some(tmpfs_file_read),
    write: Some(tmpfs_file_write),
    lseek: Some(tmpfs_file_lseek),
    close: Some(tmpfs_file_close),
    poll: None,
};

// SAFETY: VFS callback contract — pointers are valid for the call.
unsafe fn tmpfs_get_file_ops(inode: &Inode) -> Option<&'static crate::fs::file::FileOps> {
    if inode.mode.is_directory() {
        Some(&crate::fs::file::DIR_FILE_OPS)
    } else if inode.mode.is_regular_file() {
        Some(&TMPFS_FILE_OPS)
    } else {
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
    mknod: None, // tmpfs does not support device nodes in this wave
    rename: Some(tmpfs_rename),
    readlink: Some(tmpfs_readlink),
    get_file_ops: Some(tmpfs_get_file_ops),
    readdir: Some(tmpfs_readdir),
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
        let sb = TmpfsSuperBlock::new(9001);
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
        let sb = TmpfsSuperBlock::new(9002);
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
        let sb = TmpfsSuperBlock::new(9003);
        let root = sb.root_node.clone();
        let d = sb.new_node(b"d".to_vec(), TmpfsType::Directory);
        root.add_child(d.clone());
        assert!(root.find_child(b"d").is_some());
        assert!(d.list_children().is_empty());
        assert!(root.remove_child(b"d"));
        assert!(root.find_child(b"d").is_none());
    }
}
