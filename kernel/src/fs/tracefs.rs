//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! tracefs — the filesystem the tracing subsystem is exposed through.
//!
//! The reference kernel mounts tracefs at `/sys/kernel/tracing` (a
//! standalone filesystem with its own magic, TRACEFS_MAGIC), and the
//! same instance is additionally reachable at
//! `/sys/kernel/debug/tracing` (the reference automounts tracefs under
//! debugfs). Rux does the same: one tracefs instance, mounted at both
//! paths, so consumers of either path keep working.
//!
//! The tree is kernel-owned (userspace reads and, where the mode
//! permits, writes — it cannot create or remove entries), and file
//! contents are regenerated on every `open()`, like the reference's
//! seq_file-backed tracefs files. The node type is shared with debugfs
//! (`DebugFsNode`): both filesystems are kernel-owned attribute trees,
//! and sharing the node keeps the registration API identical.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicPtr, AtomicU64, Ordering};

use crate::errno;
use crate::fs::debugfs::DebugFsNode;
use crate::fs::inode::{file_type, Ino, Inode, InodeMode, INodeOps, VfsDirEntry};
use crate::fs::mount::{MntFlags, VfsMount};
use crate::fs::superblock::{FileSystemType, SuperBlock};
use crate::sync::spinlock::Spinlock;

/// tracefs magic number (the reference: TRACEFS_MAGIC).
const TRACEFS_MAGIC: u32 = 0x7472_6163;

/// icache identity tag for tracefs inodes (VFS-H8: (ino, fs_id) must
/// never collide across filesystems; rootfs/procfs/devfs/cgroup take
/// 0001-0004, debugfs takes 0005).
pub const FS_ID_TRACEFS: u64 = 0x5452_4143_0006;

// ============================================================================
// Tree
// ============================================================================

static ROOT: Spinlock<Option<Arc<DebugFsNode>>> = Spinlock::new(None);
static INO_COUNTER: AtomicU64 = AtomicU64::new(2);

fn next_ino() -> u64 {
    INO_COUNTER.fetch_add(1, Ordering::Relaxed)
}

/// The tracefs root node (None before `init_tracefs`).
pub fn tracefs_root() -> Option<Arc<DebugFsNode>> {
    ROOT.lock().clone()
}

// ============================================================================
// Population API (kernel side)
// ============================================================================

/// Create a directory. `path` is absolute inside tracefs, e.g. `"/options"`.
/// Intermediate directories are created on the way.
///
/// Returns false when the path already exists as a non-directory.
pub fn create_dir(path: &str) -> bool {
    let root = match tracefs_root() {
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
/// handler. `path` is absolute inside tracefs, e.g. `"/tracing_on"`.
///
/// The generator runs on every `open()`, so the file always shows the
/// live value. The store handler runs on `write()` and returns 0 on
/// success or a negative errno.
pub fn create_file<F, S>(path: &str, mode: u32, show: F, store: Option<S>) -> bool
where
    F: Fn() -> Vec<u8> + Send + Sync + 'static,
    S: Fn(&[u8]) -> i32 + Send + Sync + 'static,
{
    let root = match tracefs_root() {
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

/// tracefs superblock wrapper.
pub struct TraceFsSuperBlock {
    /// base
    pub sb: SuperBlock,
    /// tree root
    pub root: Arc<DebugFsNode>,
}

static GLOBAL_SB: AtomicPtr<TraceFsSuperBlock> = AtomicPtr::new(core::ptr::null_mut());

/// Mount callback used by the filesystem registry (userspace
/// `mount("tracefs", ...)`).
/// SAFETY: FsContext is a valid VFS mount callback argument.
unsafe extern "C" fn tracefs_mount_cb(
    _fs_context: &crate::fs::superblock::FsContext<'_>,
) -> Result<*mut SuperBlock, i32> {
    let root = tracefs_root().ok_or(errno::Errno::InvalidArgument.as_neg_i32())?;
    let sb = Box::new(TraceFsSuperBlock {
        sb: SuperBlock::new(4096, TRACEFS_MAGIC),
        root,
    });
    Ok(Box::into_raw(sb) as *mut SuperBlock)
}

/// Superblock teardown.
/// SAFETY: VFS callback contract.
unsafe extern "C" fn tracefs_kill_sb(_sb: *mut SuperBlock) {}

/// The tracefs filesystem type.
pub static TRACEFS_FS_TYPE: FileSystemType = FileSystemType::new(
    "tracefs",
    Some(tracefs_mount_cb),
    Some(tracefs_kill_sb),
    0,
);

/// Build the tracefs tree and register the filesystem type.
pub fn init_tracefs() -> Result<(), i32> {
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

    let sb = Box::new(TraceFsSuperBlock {
        sb: SuperBlock::new(4096, TRACEFS_MAGIC),
        root: root.clone(),
    });
    GLOBAL_SB.store(Box::into_raw(sb), Ordering::Release);
    *ROOT.lock() = Some(root);

    let _ = register_filesystem(&TRACEFS_FS_TYPE);
    Ok(())
}

/// Mount the initialized instance at one path. `mountpoint` must already
/// exist as a directory in the dentry tree (the caller resolves it the
/// way the debugfs boot mount does).
pub fn mount_at(mountpoint: &str) -> Result<(), i32> {
    if GLOBAL_SB.load(Ordering::Acquire).is_null() {
        return Err(errno::Errno::InvalidArgument.as_neg_i32());
    }
    // The mount point must already exist WITH an inode: vfs_mount walks
    // the path and would otherwise create missing components as bare
    // dentries, shadowing the underlying sysfs directory (the same
    // dance the debugfs boot mount performs).
    let _ = crate::fs::vfs::path_lookup(mountpoint, 0);
    crate::fs::vfs::vfs_mount(mountpoint, create_root_inode(), MntFlags::new(0));
    crate::fs::mount::register_mount("tracefs", mountpoint, "tracefs", "rw");
    Ok(())
}

/// Root inode for the mounted tracefs instance.
pub fn create_root_inode() -> Arc<Inode> {
    let root = match tracefs_root() {
        Some(r) => r,
        None => {
            let mut inode = Inode::new(1, InodeMode::new(InodeMode::S_IFDIR | 0o755));
            inode.fs_id = FS_ID_TRACEFS;
            inode.ops = Some(&TRACEFS_INODE_OPS);
            return Arc::new(inode);
        }
    };
    let entry = Box::new(TraceFsEntryData { node: root });
    let mut inode = Inode::new(1, InodeMode::new(InodeMode::S_IFDIR | 0o755));
    inode.fs_id = FS_ID_TRACEFS;
    inode.ops = Some(&TRACEFS_INODE_OPS);
    inode.private_data = Some(Box::into_raw(entry) as *mut u8);
    Arc::new(inode)
}

// ============================================================================
// Inode operations
// ============================================================================

/// Per-inode tracefs data (leaked into Inode::private_data, reclaimed
/// by tracefs_destroy_inode). Same shape as the debugfs entry: both
/// filesystems are kernel-owned trees of `DebugFsNode`.
pub struct TraceFsEntryData {
    /// the node this inode stands for
    pub node: Arc<DebugFsNode>,
}

/// Recover the entry data from an inode.
/// SAFETY: private_data must have come from tracefs_iget/create_root_inode.
unsafe fn entry_of(inode: &Inode) -> Option<&'static TraceFsEntryData> {
    inode
        .private_data
        .map(|p| &*(p as *const TraceFsEntryData))
}

/// Resolve a child of `dir`.
fn resolve_child(dir: &DebugFsNode, name: &[u8]) -> Option<Arc<DebugFsNode>> {
    if !dir.is_dir {
        return None;
    }
    let key = core::str::from_utf8(name).ok()?;
    dir.children.lock().get(key).cloned()
}

/// tracefs lookup: child inode number.
/// SAFETY: VFS callback contract.
unsafe fn tracefs_lookup(dir: &Inode, name: &[u8]) -> Result<Ino, errno::Errno> {
    let entry = entry_of(dir).ok_or(errno::Errno::NotADirectory)?;
    match resolve_child(&entry.node, name) {
        Some(child) => Ok(child.ino),
        None => Err(errno::Errno::NoSuchFileOrDirectory),
    }
}

/// tracefs iget: instantiate the VFS Inode for a resolved child.
/// SAFETY: VFS callback contract.
unsafe fn tracefs_iget(
    parent: &Inode,
    name: &[u8],
    ino: Ino,
) -> Result<Arc<Inode>, errno::Errno> {
    let parent_entry = entry_of(parent).ok_or(errno::Errno::NotADirectory)?;
    let node = resolve_child(&parent_entry.node, name)
        .ok_or(errno::Errno::NoSuchFileOrDirectory)?;

    let mode = if node.is_dir {
        InodeMode::new(InodeMode::S_IFDIR | 0o755)
    } else {
        InodeMode::new(InodeMode::S_IFREG | node.mode)
    };

    let entry = Box::new(TraceFsEntryData { node });
    let mut inode = Inode::new(ino, mode);
    inode.fs_id = FS_ID_TRACEFS;
    inode.ops = Some(&TRACEFS_INODE_OPS);
    inode.private_data = Some(Box::into_raw(entry) as *mut u8);
    Ok(Arc::new(inode))
}

/// tracefs getattr.
/// SAFETY: VFS callback contract.
unsafe fn tracefs_getattr(inode: &Inode, stat: &mut crate::fs::Stat) -> i32 {
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

/// tracefs setattr: ATTR_SIZE is a no-op (the guest stdio writes the
/// value and the newline as separate write() calls, so O_TRUNC must not
/// fail); everything else is EPERM — the tree is kernel-owned.
/// SAFETY: VFS callback contract.
unsafe fn tracefs_setattr(_inode: &Inode, attr: u32, _arg1: u64, _arg2: u64) -> i32 {
    if attr == crate::fs::inode::setattr_attr::ATTR_SIZE {
        return 0;
    }
    errno::Errno::OperationNotPermitted.as_neg_i32()
}

/// tracefs readdir: the child entries of a directory.
/// SAFETY: VFS callback contract.
unsafe fn tracefs_readdir(inode: &Inode) -> Option<Vec<VfsDirEntry>> {
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

/// tracefs open: snapshot the file content into file-private storage
/// (the generator runs here, so every open sees the live value).
/// SAFETY: VFS callback contract.
unsafe fn tracefs_open(inode: &Inode, file: &crate::fs::File) -> i32 {
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
    let snapshot = Box::new(TraceFsFileContent { data: content });
    file.set_private_data(Box::into_raw(snapshot) as *mut u8);
    0
}

/// tracefs get_file_ops.
/// SAFETY: VFS callback contract.
unsafe fn tracefs_get_file_ops(inode: &Inode) -> Option<&'static crate::fs::file::FileOps> {
    if inode.mode.is_regular_file() {
        Some(&TRACEFS_FILE_OPS)
    } else if inode.mode.is_directory() {
        Some(&crate::fs::file::DIR_FILE_OPS)
    } else {
        None
    }
}

/// tracefs destroy_inode: reclaim the leaked Box<TraceFsEntryData>.
/// SAFETY: VFS callback contract; Drop-time hook.
unsafe fn tracefs_destroy_inode(inode: &mut Inode) {
    if let Some(ptr) = inode.private_data.take() {
        drop(Box::from_raw(ptr as *mut TraceFsEntryData));
    }
}

/// tracefs inode operations table.
pub static TRACEFS_INODE_OPS: INodeOps = INodeOps {
    lookup: Some(tracefs_lookup),
    create: None, // tracefs is kernel-owned
    link: None,
    unlink: None,
    symlink: None,
    mkdir: None,
    rmdir: None,
    mknod: None,
    rename: None,
    readlink: None,
    get_file_ops: Some(tracefs_get_file_ops),
    readdir: Some(tracefs_readdir),
    readdir_cacheable: false,
    open: Some(tracefs_open),
    permission: None, // default DAC path
    getattr: Some(tracefs_getattr),
    setattr: Some(tracefs_setattr),
    iget: Some(tracefs_iget),
    destroy_inode: Some(tracefs_destroy_inode),
};

// ============================================================================
// File operations
// ============================================================================

/// File content snapshot (File::private_data while open).
pub struct TraceFsFileContent {
    /// generated content
    pub data: Vec<u8>,
}

/// read: serve from the open-time snapshot at the file position.
fn tracefs_file_read(file: &crate::fs::File, buf: &mut [u8]) -> isize {
    // SAFETY: private_data was installed by tracefs_open under this File.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return errno::Errno::BadFileNumber.as_neg_i32() as isize,
    };
    // SAFETY: pointer provenance is Box::into_raw in tracefs_open.
    let content = unsafe { &*(ptr as *const TraceFsFileContent) };
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
fn tracefs_file_write(file: &crate::fs::File, buf: &[u8]) -> isize {
    // SAFETY: inode pointer set at open; read-only access here.
    let inode = match unsafe { (*file.inode.get()).clone() } {
        Some(i) => i,
        None => return errno::Errno::BadFileNumber.as_neg_i32() as isize,
    };
    // SAFETY: inode provenance is the tracefs iget path.
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
fn tracefs_file_lseek(file: &crate::fs::File, offset: isize, whence: i32) -> isize {
    // SAFETY: see tracefs_file_read.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return errno::Errno::BadFileNumber.as_neg_i32() as isize,
    };
    // SAFETY: pointer provenance is Box::into_raw in tracefs_open.
    let content = unsafe { &*(ptr as *const TraceFsFileContent) };
    let file_size = content.data.len() as isize;
    let new_offset = match whence {
        0 => offset,
        1 => match (file.get_pos() as isize).checked_add(offset) {
            Some(v) => v,
            None => return errno::Errno::InvalidArgument.as_neg_i32() as isize,
        },
        2 => match file_size.checked_add(offset) {
            Some(v) => v,
            None => return errno::Errno::InvalidArgument.as_neg_i32() as isize,
        },
        _ => return errno::Errno::InvalidArgument.as_neg_i32() as isize,
    };
    if new_offset < 0 || new_offset > file_size {
        return errno::Errno::InvalidArgument.as_neg_i32() as isize;
    }
    file.set_pos(new_offset as u64);
    new_offset
}

/// close: free the content snapshot.
fn tracefs_file_close(file: &crate::fs::File) -> i32 {
    // SAFETY: see tracefs_file_read; null-out first to close the race window.
    let ptr = unsafe { (*file.private_data.get()).take() };
    if let Some(ptr) = ptr {
        // SAFETY: provenance is Box::into_raw in tracefs_open.
        drop(unsafe { Box::from_raw(ptr as *mut TraceFsFileContent) });
    }
    0
}

/// tracefs file operations.
pub static TRACEFS_FILE_OPS: crate::fs::FileOps = crate::fs::FileOps::builder()
    .read(tracefs_file_read)
    .write(tracefs_file_write)
    .lseek(tracefs_file_lseek)
    .close(tracefs_file_close)
    .build();
