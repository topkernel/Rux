//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! devfs - Device Filesystem
//!
//! - Mounted at /dev
//! - Manages device nodes
//! - Supports character devices and block devices

pub mod registry;

use alloc::string::String;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;
use alloc::sync::Arc;
use crate::sync::spinlock::Spinlock;
use crate::fs::file::FileOps;
use super::dev_t::DevNo;

// Re-export device number definitions
pub use super::dev_t;

// ============================================================================
// devfs directory entries
// ============================================================================

/// devfs directory entry type
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DevEntryType {
    /// Directory
    Directory,
    /// Character device
    CharDevice,
    /// Block device (not implemented)
    BlockDevice,
}

/// devfs directory entry
pub struct DevfsEntry {
    /// Name
    pub name: String,
    /// Type
    pub entry_type: DevEntryType,
    /// Child entries (valid only for directory type)
    pub children: Spinlock<BTreeMap<String, Arc<DevfsEntry>>>,
    /// Device number (valid only for device types)
    pub devno: DevNo,
    /// Permissions (default 0666)
    pub mode: u32,
}

impl DevfsEntry {
    /// Create directory
    pub fn new_dir(name: &str) -> Self {
        Self {
            name: String::from(name),
            entry_type: DevEntryType::Directory,
            children: Spinlock::new(BTreeMap::new()),
            devno: DevNo::default(),
            mode: 0o755,
        }
    }

    /// Create character device
    pub fn new_char_device(name: &str, devno: DevNo) -> Self {
        Self {
            name: String::from(name),
            entry_type: DevEntryType::CharDevice,
            children: Spinlock::new(BTreeMap::new()),
            devno,
            mode: 0o666,
        }
    }

    /// Create character device with custom permissions
    pub fn new_char_device_with_mode(name: &str, devno: DevNo, mode: u32) -> Self {
        Self {
            name: String::from(name),
            entry_type: DevEntryType::CharDevice,
            children: Spinlock::new(BTreeMap::new()),
            devno,
            mode: mode & 0o777,
        }
    }

    /// Is directory
    pub fn is_dir(&self) -> bool {
        self.entry_type == DevEntryType::Directory
    }

    /// Is character device
    pub fn is_char_device(&self) -> bool {
        self.entry_type == DevEntryType::CharDevice
    }
}

// ============================================================================
// devfs filesystem
// ============================================================================

/// devfs global instance
static DEVFS_ROOT: Spinlock<Option<Arc<DevfsEntry>>> = Spinlock::new(None);

/// Initialize devfs
pub fn init() {
    let mut root = DEVFS_ROOT.lock_irqsave();

    // Create root directory
    let root_entry = Arc::new(DevfsEntry::new_dir("dev"));

    // Create /dev/input directory
    let input_dir = Arc::new(DevfsEntry::new_dir("input"));

    // Add input to root directory
    root_entry.children.lock_irqsave().insert(String::from("input"), input_dir);

    *root = Some(root_entry);
}

/// Create device node
///
/// # Arguments
/// - path: Device path (e.g., "/input/event0")
/// - devno: Device number
/// - mode: File mode (S_IFCHR, etc.)
///
/// # Returns
/// Ok(()) on success, Err(()) on failure
pub fn mknod(path: &str, devno: DevNo, mode: u32) -> Result<(), ()> {
    // Remove leading /
    let path = path.strip_prefix('/').unwrap_or(path);

    if path.is_empty() {
        return Err(());
    }

    // Collect path components into stack array (avoid Vec allocation)
    const MAX_COMPONENTS: usize = 16;
    let mut components: [&str; MAX_COMPONENTS] = [""; MAX_COMPONENTS];
    let mut ncomponents: usize = 0;
    for part in path.split('/').filter(|s| !s.is_empty()) {
        if ncomponents >= MAX_COMPONENTS {
            return Err(());
        }
        components[ncomponents] = part;
        ncomponents += 1;
    }
    if ncomponents == 0 {
        return Err(());
    }

    let root = DEVFS_ROOT.lock_irqsave();
    let root = match root.as_ref() {
        Some(r) => r,
        None => return Err(()),
    };

    // Traverse to parent of last component
    let mut current = root.clone();
    let parent_count = ncomponents - 1;  // ncomponents >= 1 guaranteed above
    for i in 0..parent_count {
        let component = components[i];
        let children = current.children.lock_irqsave();
        match children.get(component) {
            Some(child) => {
                let child = child.clone();
                drop(children);
                current = child;
            }
            None => return Err(()),
        }
    }

    // Create device node
    let device_name = components[ncomponents - 1];
    let entry = Arc::new(DevfsEntry::new_char_device_with_mode(device_name, devno, mode));
    current.children.lock_irqsave().insert(String::from(device_name), entry);

    Ok(())
}

/// Create directory
pub fn mkdir(path: &str) -> Result<(), ()> {
    // Remove leading /
    let path = path.strip_prefix('/').unwrap_or(path);

    crate::dfx::sbi_debug::sbi_dbg(":mkdir_path=[");
    crate::dfx::sbi_debug::sbi_dbg(path);
    crate::dfx::sbi_debug::sbi_dbg("]\n");

    if path.is_empty() {
        return Err(());
    }

    let root = DEVFS_ROOT.lock_irqsave();
    let root = match root.as_ref() {
        Some(r) => r,
        None => return Err(()),
    };

    // Parse path
    let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    let ncomponents = components.len();
    if ncomponents == 0 {
        return Err(());
    }

    // Traverse to parent of last component
    let mut current = root.clone();
    let parent_count = ncomponents - 1;  // ncomponents >= 1 guaranteed above
    for i in 0..parent_count {
        let component = components[i];
        let children = current.children.lock_irqsave();
        match children.get(component) {
            Some(child) => {
                let child = child.clone();
                drop(children);
                current = child;
            }
            None => return Err(()),
        }
    }

    // Create directory
    let dir_name = components.last().unwrap();
    let entry = Arc::new(DevfsEntry::new_dir(dir_name));

    current.children.lock_irqsave().insert(String::from(*dir_name), entry);

    Ok(())
}

/// Lookup path
///
/// # Returns
/// Returns (entry, is_char_device, devno) if found
pub fn lookup(path: &str) -> Option<(Arc<DevfsEntry>, bool, DevNo)> {
    // Remove leading /
    let path = path.strip_prefix('/').unwrap_or(path);

    // Empty path or "." means root directory
    if path.is_empty() || path == "." {
        // Return root directory
        let root = DEVFS_ROOT.lock_irqsave();
        let root = root.as_ref()?;
        return Some((root.clone(), false, DevNo::default()));
    }

    let root = DEVFS_ROOT.lock_irqsave();
    let root = root.as_ref()?;

    // Parse path, filter out "." and ".."
    let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty() && *s != "." && *s != "..").collect();

    // If filtered result is empty, return root directory
    if components.is_empty() {
        return Some((root.clone(), false, DevNo::default()));
    }

    // Traverse path
    let mut current = root.clone();
    for component in &components {
        let children = current.children.lock_irqsave();
        match children.get(*component) {
            Some(child) => {
                let child = child.clone();
                drop(children);
                current = child;
            }
            None => return None,
        }
    }

    Some((
        current.clone(),
        current.is_char_device(),
        current.devno,
    ))
}

/// Check if devfs is initialized
pub fn is_mounted() -> bool {
    DEVFS_ROOT.lock_irqsave().is_some()
}

/// Get the devfs root entry (for dentry tree mount).
pub fn get_root_entry() -> Option<Arc<DevfsEntry>> {
    DEVFS_ROOT.lock_irqsave().clone()
}

/// Directory entry info (name, is_dir, ino)
pub type DevfsDirEntry = (String, bool, u64);

/// List directory contents
///
/// # Arguments
/// - path: devfs internal path (e.g., "" for root directory, "input" for /dev/input)
///
/// # Returns
/// Returns directory entry list on success, None on failure
pub fn list_dir(path: &str) -> Option<Vec<DevfsDirEntry>> {
    let root = DEVFS_ROOT.lock_irqsave();
    let root = root.as_ref()?;

    // Empty path or "." means root directory
    if path.is_empty() || path == "/" || path == "." {
        let children = root.children.lock_irqsave();
        let mut entries = Vec::new();
        let mut ino = 1u64;
        for (name, entry) in children.iter() {
            entries.push((name.clone(), entry.is_dir(), ino));
            ino += 1;
        }
        return Some(entries);
    }

    // Parse path, filter out "." and ".."
    let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty() && *s != "." && *s != "..").collect();

    // Traverse to target directory
    let mut current = root.clone();
    for component in &components {
        let children = current.children.lock_irqsave();
        match children.get(*component) {
            Some(child) => {
                let child = child.clone();
                drop(children);
                current = child;
            }
            None => return None,
        }
    }

    // Check if directory
    if !current.is_dir() {
        return None;
    }

    // List children
    let children = current.children.lock_irqsave();
    let mut entries = Vec::new();
    let mut ino = 1u64;
    for (name, entry) in children.iter() {
        entries.push((name.clone(), entry.is_dir(), ino));
        ino += 1;
    }
    Some(entries)
}

/// Get device path (check if under /dev)
///
/// If path starts with /dev, returns devfs path (with /dev prefix removed)
pub fn parse_dev_path(path: &str) -> Option<&str> {
    if path == "/dev" {
        return Some("");
    }
    if path.starts_with("/dev/") {
        return Some(&path[5..]);
    }
    None
}

// ============================================================================
// DevFS Inode Operations (for VFS dentry tree integration)
// ============================================================================

use crate::fs::inode::{Inode, InodeMode, Ino, INodeOps};
use crate::errno;

/// Devfs lookup: given a parent directory inode and a child name, return child's ino.
/// We use a simple hash of the name as the inode number since devfs has no real inodes.
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn devfs_lookup(dir: &Inode, name: &[u8]) -> Result<Ino, i32> {
    let entry_ptr = dir.private_data.ok_or(errno::Errno::NotADirectory.as_neg_i32())?;
    let entry = &*(entry_ptr as *const DevfsEntry);

    if !entry.is_dir() {
        return Err(errno::Errno::NotADirectory.as_neg_i32());
    }

    let name_str = core::str::from_utf8(name)
        .map_err(|_| errno::Errno::InvalidArgument.as_neg_i32())?;

    let children = entry.children.lock_irqsave();
    if let Some(child) = children.get(name_str) {
        // Use a simple hash as inode number
        Ok(devfs_ino_hash(name_str))
    } else {
        Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
    }
}

/// Devfs iget: instantiate a VFS Inode from (parent_inode, name, child_ino).
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn devfs_iget(parent: &Inode, name: &[u8], _ino: Ino) -> Result<alloc::sync::Arc<Inode>, i32> {
    let entry_ptr = parent.private_data.ok_or(errno::Errno::NotADirectory.as_neg_i32())?;
    let parent_entry = &*(entry_ptr as *const DevfsEntry);

    let name_str = core::str::from_utf8(name)
        .map_err(|_| errno::Errno::InvalidArgument.as_neg_i32())?;

    let children = parent_entry.children.lock_irqsave();
    let child = children.get(name_str)
        .ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;
    let child = child.clone();
    drop(children);

    let mode = if child.is_dir() {
        InodeMode::new(InodeMode::S_IFDIR | child.mode)
    } else if child.is_char_device() {
        InodeMode::new(InodeMode::S_IFCHR | child.mode)
    } else {
        InodeMode::new(InodeMode::S_IFBLK | child.mode)
    };

    let ino = devfs_ino_hash(name_str);
    let mut inode = Inode::new(ino, mode);
    inode.ops = Some(&DEVFS_INODE_OPS);
    inode.private_data = Some(Arc::as_ptr(&child) as *mut u8);
    Ok(alloc::sync::Arc::new(inode))
}

/// Devfs getattr: fill stat for a devfs entry.
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn devfs_getattr(inode: &Inode, stat: &mut crate::fs::Stat) -> i32 {
    let entry_ptr = match inode.private_data {
        Some(ptr) => ptr,
        None => return errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    };
    let entry = &*(entry_ptr as *const DevfsEntry);

    stat.st_dev = 0;
    stat.st_ino = inode.ino;
    stat.st_nlink = 1;
    stat.st_uid = 0;
    stat.st_gid = 0;
    stat.st_rdev = ((entry.devno.major as u64) << 32) | (entry.devno.minor as u64);
    stat.st_size = 0;
    stat.st_blocks = 0;
    stat.st_blksize = 4096;
    stat.st_mode = inode.mode.bits();
    stat.st_atime = 0;
    stat.st_atime_nsec = 0;
    stat.st_mtime = 0;
    stat.st_mtime_nsec = 0;
    stat.st_ctime = 0;
    stat.st_ctime_nsec = 0;
    0
}

/// Simple hash for devfs inode numbers (devfs has no real on-disk inodes).
fn devfs_ino_hash(name: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in name.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    // Ensure non-zero
    if hash == 0 { 1 } else { hash }
}

/// DevFS get_file_ops: return device-specific ops for char devices, DIR_FILE_OPS for directories
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn devfs_get_file_ops(inode: &Inode) -> Option<&'static crate::fs::file::FileOps> {
    if inode.mode.is_char_device() {
        let entry_ptr = inode.private_data?;
        let entry = &*(entry_ptr as *const DevfsEntry);
        registry::get_char_device_ops(entry.devno)
    } else if inode.mode.is_directory() {
        Some(&crate::fs::file::DIR_FILE_OPS)
    } else {
        None
    }
}

/// DevFS readdir: list directory entries
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn devfs_readdir(inode: &Inode) -> Option<alloc::vec::Vec<crate::fs::inode::VfsDirEntry>> {
    use crate::fs::inode::file_type;

    let entry_ptr = inode.private_data?;
    let entry = &*(entry_ptr as *const DevfsEntry);
    if !entry.is_dir() {
        return None;
    }
    let children = entry.children.lock_irqsave();
    let mut entries = alloc::vec::Vec::new();
    let mut ino = 1u64;
    for (name, child) in children.iter() {
        let dt = if child.is_dir() {
            file_type::DT_DIR
        } else if child.is_char_device() {
            file_type::DT_CHR
        } else {
            file_type::DT_UNKNOWN
        };
        entries.push(crate::fs::inode::VfsDirEntry {
            ino,
            name: name.as_bytes().to_vec(),
            file_type: dt,
        });
        ino += 1;
    }
    Some(entries)
}

/// DevFS inode operations table
pub static DEVFS_INODE_OPS: INodeOps = INodeOps {
    lookup: Some(devfs_lookup),
    create: None,
    link: None,
    unlink: None,
    symlink: None,
    mkdir: None,
    rmdir: None,
    mknod: None,
    rename: None,
    readlink: None,
    get_file_ops: Some(devfs_get_file_ops),
    readdir: Some(devfs_readdir),
    open: None,
    permission: None,
    getattr: Some(devfs_getattr),
    setattr: None,
    iget: Some(devfs_iget),
};

/// Create a VFS inode for the devfs root entry.
/// Called during mount to set up the root dentry's inode.
pub fn create_root_inode(root_entry: &Arc<DevfsEntry>) -> alloc::sync::Arc<Inode> {
    let mut inode = Inode::new(1, InodeMode::new(InodeMode::S_IFDIR | 0o755));
    inode.ops = Some(&DEVFS_INODE_OPS);
    inode.private_data = Some(Arc::as_ptr(root_entry) as *mut u8);
    alloc::sync::Arc::new(inode)
}
