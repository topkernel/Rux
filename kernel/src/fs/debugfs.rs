//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! debugfs — the kernel debug filesystem.
//!
//! Linux mounts debugfs at `/sys/kernel/debug` and uses it for exactly
//! one purpose: exposing kernel-internal state and runtime knobs that are
//! deliberately NOT part of the stable userspace ABI. Rux uses the same
//! mount point and the same shape:
//!
//! ```text
//! /sys/kernel/debug/
//! └── dfx/
//!     ├── taskdump      Y|N   (0644, writable)
//!     └── ...
//! ```
//!
//! Semantics copied from Linux:
//!
//! - The tree is **kernel-owned**: userspace may read entries and, when
//!   the file mode permits it, write a new value. It cannot create,
//!   rename or remove entries.
//! - A knob file is regenerated on every `open()`, so `cat` always shows
//!   the current value (Linux debugfs uses `seq_file` for the same
//!   reason).
//! - Directories are listable (`readdir`), which is how a user learns
//!   which knobs exist — there is no separate "index" file.
//! - The mount point `/sys/kernel/debug` is a real sysfs directory, so
//!   the mount has somewhere to land (Linux does the same: sysfs owns
//!   the directory, debugfs is mounted over it).

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::errno;
use crate::fs::inode::{file_type, Ino, Inode, InodeMode, INodeOps, VfsDirEntry};
use crate::fs::mount::{MntFlags, VfsMount};
use crate::fs::superblock::{FileSystemType, SuperBlock};
use crate::sync::spinlock::Spinlock;

/// debugfs magic number (Linux: STATFS_DEBUGFS_MAGIC).
const DEBUGFS_MAGIC: u32 = 0x6462_6720;

/// icache identity tag for debugfs inodes (VFS-H8: (ino, fs_id) must
/// never collide across filesystems; rootfs/procfs/devfs/sysfs take
/// 0001-0004).
pub const FS_ID_DEBUGFS: u64 = 0x4445_4247_0005;

// ============================================================================
// Tree
// ============================================================================

/// A debugfs node: either a directory or a regular file with an
/// optional read generator and an optional write handler.
pub struct DebugFsNode {
    /// entry name ("" for the root)
    pub name: String,
    /// fs-unique inode number
    pub ino: u64,
    /// directory or regular file
    pub is_dir: bool,
    /// file mode bits for regular files (0644 = root-writable)
    pub mode: u32,
    /// child nodes (BTreeMap → readdir order is deterministic)
    pub children: Spinlock<BTreeMap<String, Arc<DebugFsNode>>>,
    /// read generator — called with NO debugfs locks held
    pub show: Option<Box<dyn Fn() -> Vec<u8> + Send + Sync>>,
    /// write handler — returns 0 or a negative errno
    pub store: Option<Box<dyn Fn(&[u8]) -> i32 + Send + Sync>>,
}

// SAFETY: every mutable field sits behind a Spinlock; the closures are
// required to be Send + Sync at construction.
unsafe impl Send for DebugFsNode {}
unsafe impl Sync for DebugFsNode {}

static ROOT: Spinlock<Option<Arc<DebugFsNode>>> = Spinlock::new(None);
static INO_COUNTER: AtomicU64 = AtomicU64::new(2);

fn next_ino() -> u64 {
    INO_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// The debugfs root node (None before `init_debugfs`).
pub fn debugfs_root() -> Option<Arc<DebugFsNode>> {
    ROOT.lock().clone()
}

// ============================================================================
// Population API (kernel side)
// ============================================================================

/// Create a directory. `path` is absolute inside debugfs, e.g.
/// `"/dfx"`. Intermediate directories are created on the way.
///
/// Returns false when the path already exists as a non-directory.
pub fn create_dir(path: &str) -> bool {
    let root = match debugfs_root() {
        Some(r) => r,
        None => return false,
    };
    let mut current = root;
    for component in path.split('/').filter(|c| !c.is_empty()) {
        let existing = {
            let children = current.children.lock();
            children.get(component).cloned()
        };
        if let Some(existing) = existing {
            if !existing.is_dir {
                return false;
            }
            current = existing;
            continue;
        }
        let node = Arc::new(DebugFsNode {
            name: String::from(component),
            ino: next_ino(),
            is_dir: true,
            mode: 0o755,
            children: Spinlock::new(BTreeMap::new()),
            show: None,
            store: None,
        });
        let mut children = current.children.lock();
        children.insert(String::from(component), node.clone());
        drop(children);
        current = node;
    }
    true
}

/// Create a regular file with a read generator and an optional write
/// handler. `path` is absolute inside debugfs, e.g. `"/dfx/taskdump"`.
///
/// The generator runs on every `open()`, so the file always shows the
/// live value. The store handler runs on `write()` and returns 0 on
/// success or a negative errno.
pub fn create_file<F, S>(path: &str, mode: u32, show: F, store: Option<S>) -> bool
where
    F: Fn() -> Vec<u8> + Send + Sync + 'static,
    S: Fn(&[u8]) -> i32 + Send + Sync + 'static,
{
    let root = match debugfs_root() {
        Some(r) => r,
        None => return false,
    };
    let mut components: Vec<&str> = path.split('/').filter(|c| !c.is_empty()).collect();
    let file_name = match components.pop() {
        Some(n) => n,
        None => return false,
    };
    let mut current = root;
    for component in components {
        let existing = {
            let children = current.children.lock();
            children.get(component).cloned()
        };
        if let Some(existing) = existing {
            if !existing.is_dir {
                return false;
            }
            current = existing;
            continue;
        }
        let node = Arc::new(DebugFsNode {
            name: String::from(component),
            ino: next_ino(),
            is_dir: true,
            mode: 0o755,
            children: Spinlock::new(BTreeMap::new()),
            show: None,
            store: None,
        });
        let mut children = current.children.lock();
        children.insert(String::from(component), node.clone());
        drop(children);
        current = node;
    }
    let node = Arc::new(DebugFsNode {
        name: String::from(file_name),
        ino: next_ino(),
        is_dir: false,
        mode,
        children: Spinlock::new(BTreeMap::new()),
        show: Some(Box::new(show)),
        store: store.map(|s| Box::new(s) as Box<dyn Fn(&[u8]) -> i32 + Send + Sync>),
    });
    let mut children = current.children.lock();
    children.insert(String::from(file_name), node);
    true
}

// ============================================================================
// Superblock / mount
// ============================================================================

/// debugfs superblock wrapper.
pub struct DebugFsSuperBlock {
    /// base
    pub sb: SuperBlock,
    /// tree root
    pub root: Arc<DebugFsNode>,
}

static GLOBAL_SB: core::sync::atomic::AtomicPtr<DebugFsSuperBlock> =
    core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());
static GLOBAL_MOUNT: core::sync::atomic::AtomicPtr<VfsMount> =
    core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

/// Mount callback used by the filesystem registry (userspace
/// `mount("debugfs", ...)`). The boot path mounts the instance directly,
/// like sysfs and procfs do.
/// SAFETY: FsContext is a valid VFS mount callback argument.
unsafe extern "C" fn debugfs_mount_cb(
    _fs_context: &crate::fs::superblock::FsContext<'_>,
) -> Result<*mut SuperBlock, i32> {
    let root = debugfs_root().ok_or(errno::Errno::InvalidArgument.as_neg_i32())?;
    let sb = Box::new(DebugFsSuperBlock {
        sb: SuperBlock::new(4096, DEBUGFS_MAGIC),
        root,
    });
    Ok(Box::into_raw(sb) as *mut SuperBlock)
}

/// Superblock teardown.
unsafe extern "C" fn debugfs_kill_sb(_sb: *mut SuperBlock) {}

/// The debugfs filesystem type.
pub static DEBUGFS_FS_TYPE: FileSystemType = FileSystemType::new(
    "debugfs",
    Some(debugfs_mount_cb),
    Some(debugfs_kill_sb),
    0,
);

/// Build the debugfs tree and register the filesystem type.
pub fn init_debugfs() -> Result<(), i32> {
    use crate::fs::superblock::register_filesystem;

    {
        let guard = ROOT.lock();
        if guard.is_some() {
            return Ok(());
        }
    }

    let root = Arc::new(DebugFsNode {
        name: String::new(),
        ino: 1,
        is_dir: true,
        mode: 0o755,
        children: Spinlock::new(BTreeMap::new()),
        show: None,
        store: None,
    });

    let sb = Box::new(DebugFsSuperBlock {
        sb: SuperBlock::new(4096, DEBUGFS_MAGIC),
        root: root.clone(),
    });
    GLOBAL_SB.store(Box::into_raw(sb), Ordering::Release);
    *ROOT.lock() = Some(root);

    let _ = register_filesystem(&DEBUGFS_FS_TYPE);
    Ok(())
}

/// Is debugfs mounted?
pub fn is_mounted() -> bool {
    !GLOBAL_MOUNT.load(Ordering::Acquire).is_null()
}

/// Prepare the mount point. The `/sys/kernel/debug` directory itself
/// lives in sysfs (Linux does the same); this only records the mount.
pub fn mount_debugfs() -> Result<(), i32> {
    if GLOBAL_SB.load(Ordering::Acquire).is_null() {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }
    let sb_ptr = GLOBAL_SB.load(Ordering::Acquire);
    let mount = Box::new(VfsMount::new(
        b"/sys/kernel/debug".to_vec(),
        b"/sys/kernel/debug".to_vec(),
        MntFlags::new(0),
        Some(sb_ptr as *mut u8),
    ));
    GLOBAL_MOUNT.store(Box::into_raw(mount) as *mut VfsMount, Ordering::Release);
    Ok(())
}

/// Root inode for the mounted debugfs instance.
pub fn create_root_inode() -> Arc<Inode> {
    let root = match debugfs_root() {
        Some(r) => r,
        None => {
            let mut inode = Inode::new(1, InodeMode::new(InodeMode::S_IFDIR | 0o755));
            inode.fs_id = FS_ID_DEBUGFS;
            inode.ops = Some(&DEBUGFS_INODE_OPS);
            return Arc::new(inode);
        }
    };
    let entry = Box::new(DebugFsEntryData { node: root });
    let mut inode = Inode::new(1, InodeMode::new(InodeMode::S_IFDIR | 0o755));
    inode.fs_id = FS_ID_DEBUGFS;
    inode.ops = Some(&DEBUGFS_INODE_OPS);
    inode.private_data = Some(Box::into_raw(entry) as *mut u8);
    Arc::new(inode)
}

// ============================================================================
// Inode operations
// ============================================================================

/// Per-inode debugfs data (leaked into Inode::private_data, reclaimed by
/// debugfs_destroy_inode).
pub struct DebugFsEntryData {
    /// the node this inode stands for
    pub node: Arc<DebugFsNode>,
}

/// Recover the entry data from an inode.
/// SAFETY: private_data must have come from debugfs_iget/create_root_inode.
unsafe fn entry_of(inode: &Inode) -> Option<&'static DebugFsEntryData> {
    inode.private_data.map(|p| &*(p as *const DebugFsEntryData))
}

/// Resolve a child of `dir`.
fn resolve_child(dir: &DebugFsNode, name: &[u8]) -> Option<Arc<DebugFsNode>> {
    if !dir.is_dir {
        return None;
    }
    let key = core::str::from_utf8(name).ok()?;
    dir.children.lock().get(key).cloned()
}

/// debugfs lookup: child inode number.
/// SAFETY: VFS callback contract.
unsafe fn debugfs_lookup(dir: &Inode, name: &[u8]) -> Result<Ino, errno::Errno> {
    let entry = entry_of(dir).ok_or(errno::Errno::NotADirectory)?;
    match resolve_child(&entry.node, name) {
        Some(child) => Ok(child.ino),
        None => Err(errno::Errno::NoSuchFileOrDirectory),
    }
}

/// debugfs iget: instantiate the VFS Inode for a resolved child.
/// SAFETY: VFS callback contract.
unsafe fn debugfs_iget(parent: &Inode, name: &[u8], ino: Ino) -> Result<Arc<Inode>, errno::Errno> {
    let parent_entry = entry_of(parent).ok_or(errno::Errno::NotADirectory)?;
    let node = resolve_child(&parent_entry.node, name)
        .ok_or(errno::Errno::NoSuchFileOrDirectory)?;

    let mode = if node.is_dir {
        InodeMode::new(InodeMode::S_IFDIR | 0o755)
    } else {
        InodeMode::new(InodeMode::S_IFREG | node.mode)
    };

    let entry = Box::new(DebugFsEntryData { node });
    let mut inode = Inode::new(ino, mode);
    inode.fs_id = FS_ID_DEBUGFS;
    inode.ops = Some(&DEBUGFS_INODE_OPS);
    inode.private_data = Some(Box::into_raw(entry) as *mut u8);
    Ok(Arc::new(inode))
}

/// debugfs getattr.
/// SAFETY: VFS callback contract.
unsafe fn debugfs_getattr(inode: &Inode, stat: &mut crate::fs::Stat) -> i32 {
    let entry = match entry_of(inode) {
        Some(e) => e,
        None => return errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    };
    stat.st_ino = inode.ino;
    if entry.node.is_dir {
        stat.st_mode = InodeMode::S_IFDIR | 0o755;
        stat.st_size = 4096;
        stat.st_nlink = 2;
    } else {
        stat.st_mode = InodeMode::S_IFREG | entry.node.mode;
        stat.st_size = 4096;
        stat.st_nlink = 1;
    }
    stat.st_uid = 0;
    stat.st_gid = 0;
    stat.st_rdev = 0;
    stat.st_blksize = 4096;
    stat.st_blocks = 8;
    stat.st_atime = 0;
    stat.st_atime_nsec = 0;
    stat.st_mtime = 0;
    stat.st_mtime_nsec = 0;
    stat.st_ctime = 0;
    stat.st_ctime_nsec = 0;
    0
}

/// debugfs setattr: ATTR_SIZE is a no-op (the guest stdio writes the
/// value and the newline as separate write() calls, so O_TRUNC must not
/// fail); everything else is EPERM — the tree is kernel-owned.
/// SAFETY: VFS callback contract.
unsafe fn debugfs_setattr(_inode: &Inode, attr: u32, _arg1: u64, _arg2: u64) -> i32 {
    if attr == crate::fs::inode::setattr_attr::ATTR_SIZE {
        return 0;
    }
    errno::Errno::OperationNotPermitted.as_neg_i32()
}

/// debugfs readdir: the child entries of a directory.
/// SAFETY: VFS callback contract.
unsafe fn debugfs_readdir(inode: &Inode) -> Option<Vec<VfsDirEntry>> {
    let entry = entry_of(inode)?;
    if !entry.node.is_dir {
        return None;
    }
    let mut out = Vec::new();
    let children = entry.node.children.lock();
    for (name, child) in children.iter() {
        let dt = if child.is_dir {
            file_type::DT_DIR
        } else {
            file_type::DT_REG
        };
        out.push(VfsDirEntry {
            ino: child.ino,
            name: name.as_bytes().to_vec(),
            file_type: dt,
        });
    }
    Some(out)
}

/// debugfs open: snapshot the file content into file-private storage
/// (the generator runs here, so every open sees the live value).
/// SAFETY: VFS callback contract.
unsafe fn debugfs_open(inode: &Inode, file: &crate::fs::File) -> i32 {
    if !inode.mode.is_regular_file() {
        return 0;
    }
    let entry = match entry_of(inode) {
        Some(e) => e,
        None => return 0,
    };
    let content = match entry.node.show.as_ref() {
        Some(show) => show(),
        None => Vec::new(),
    };
    let snapshot = Box::new(DebugFsFileContent { data: content });
    file.set_private_data(Box::into_raw(snapshot) as *mut u8);
    0
}

/// debugfs get_file_ops.
/// SAFETY: VFS callback contract.
unsafe fn debugfs_get_file_ops(inode: &Inode) -> Option<&'static crate::fs::file::FileOps> {
    if inode.mode.is_regular_file() {
        Some(&DEBUGFS_FILE_OPS)
    } else if inode.mode.is_directory() {
        Some(&crate::fs::file::DIR_FILE_OPS)
    } else {
        None
    }
}

/// debugfs destroy_inode: reclaim the leaked Box<DebugFsEntryData>.
/// SAFETY: VFS callback contract; Drop-time hook.
unsafe fn debugfs_destroy_inode(inode: &mut Inode) {
    if let Some(ptr) = inode.private_data.take() {
        drop(Box::from_raw(ptr as *mut DebugFsEntryData));
    }
}

/// debugfs inode operations table.
pub static DEBUGFS_INODE_OPS: INodeOps = INodeOps {
    lookup: Some(debugfs_lookup),
    create: None,      // debugfs is kernel-owned
    link: None,
    unlink: None,
    symlink: None,
    mkdir: None,
    rmdir: None,
    mknod: None,
    rename: None,
    readlink: None,
    get_file_ops: Some(debugfs_get_file_ops),
    readdir: Some(debugfs_readdir),
    readdir_cacheable: false,
    open: Some(debugfs_open),
    permission: None, // default DAC path
    getattr: Some(debugfs_getattr),
    setattr: Some(debugfs_setattr),
    iget: Some(debugfs_iget),
    destroy_inode: Some(debugfs_destroy_inode),
};

// ============================================================================
// File operations
// ============================================================================

/// File content snapshot (File::private_data while open).
pub struct DebugFsFileContent {
    /// generated content
    pub data: Vec<u8>,
}

/// read: serve from the open-time snapshot at the file position.
fn debugfs_file_read(file: &crate::fs::File, buf: &mut [u8]) -> isize {
    // SAFETY: private_data was installed by debugfs_open under this File.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return errno::Errno::BadFileNumber.as_neg_i32() as isize,
    };
    // SAFETY: pointer provenance is Box::into_raw in debugfs_open.
    let content = unsafe { &*(ptr as *const DebugFsFileContent) };
    let offset = file.get_pos() as usize;
    let available = content.data.len().saturating_sub(offset);
    let to_read = buf.len().min(available);
    if to_read > 0 {
        buf[..to_read].copy_from_slice(&content.data[offset..offset + to_read]);
        file.set_pos((offset + to_read) as u64);
        to_read as isize
    } else {
        0
    }
}

/// write: dispatch to the file's store handler.
fn debugfs_file_write(file: &crate::fs::File, buf: &[u8]) -> isize {
    // SAFETY: inode pointer set at open; read-only access here.
    let inode = match unsafe { (*file.inode.get()).clone() } {
        Some(i) => i,
        None => return errno::Errno::BadFileNumber.as_neg_i32() as isize,
    };
    // SAFETY: inode provenance is the debugfs iget path.
    let entry = match unsafe { entry_of(&inode) } {
        Some(e) => e,
        None => return errno::Errno::BadFileNumber.as_neg_i32() as isize,
    };
    match entry.node.store.as_ref() {
        Some(store) => {
            let ret = store(buf);
            if ret != 0 {
                ret as isize
            } else {
                buf.len() as isize
            }
        }
        None => errno::Errno::InvalidArgument.as_neg_i32() as isize, // read-only node
    }
}

/// lseek within the snapshot.
fn debugfs_file_lseek(file: &crate::fs::File, offset: isize, whence: i32) -> isize {
    // SAFETY: see debugfs_file_read.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return errno::Errno::BadFileNumber.as_neg_i32() as isize,
    };
    // SAFETY: pointer provenance is Box::into_raw in debugfs_open.
    let content = unsafe { &*(ptr as *const DebugFsFileContent) };
    let file_size = content.data.len() as isize;
    let new_offset = match whence {
        0 => offset,
        1 => file.get_pos() as isize + offset,
        2 => file_size + offset,
        _ => return errno::Errno::InvalidArgument.as_neg_i32() as isize,
    };
    if new_offset < 0 || new_offset > file_size {
        return errno::Errno::InvalidArgument.as_neg_i32() as isize;
    }
    file.set_pos(new_offset as u64);
    new_offset
}

/// close: free the content snapshot.
fn debugfs_file_close(file: &crate::fs::File) -> i32 {
    // SAFETY: see debugfs_file_read; null-out first to close the race window.
    let ptr = unsafe { (*file.private_data.get()).take() };
    if let Some(ptr) = ptr {
        // SAFETY: provenance is Box::into_raw in debugfs_open.
        drop(unsafe { Box::from_raw(ptr as *mut DebugFsFileContent) });
    }
    0
}

/// debugfs file operations.
pub static DEBUGFS_FILE_OPS: crate::fs::FileOps = crate::fs::FileOps::builder()
    .read(debugfs_file_read)
    .write(debugfs_file_write)
    .lseek(debugfs_file_lseek)
    .close(debugfs_file_close)
    .build();
