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
use spin::Mutex;
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
    pub children: Mutex<BTreeMap<String, Arc<DevfsEntry>>>,
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
            children: Mutex::new(BTreeMap::new()),
            devno: DevNo::default(),
            mode: 0o755,
        }
    }

    /// Create character device
    pub fn new_char_device(name: &str, devno: DevNo) -> Self {
        Self {
            name: String::from(name),
            entry_type: DevEntryType::CharDevice,
            children: Mutex::new(BTreeMap::new()),
            devno,
            mode: 0o666,
        }
    }

    /// Create character device with custom permissions
    pub fn new_char_device_with_mode(name: &str, devno: DevNo, mode: u32) -> Self {
        Self {
            name: String::from(name),
            entry_type: DevEntryType::CharDevice,
            children: Mutex::new(BTreeMap::new()),
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
static DEVFS_ROOT: Mutex<Option<Arc<DevfsEntry>>> = Mutex::new(None);

/// Initialize devfs
pub fn init() {
    let mut root = DEVFS_ROOT.lock();

    // Create root directory
    let root_entry = Arc::new(DevfsEntry::new_dir("dev"));

    // Create /dev/input directory
    let input_dir = Arc::new(DevfsEntry::new_dir("input"));

    // Add input to root directory
    root_entry.children.lock().insert(String::from("input"), input_dir);

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

    let root = DEVFS_ROOT.lock();
    let root = match root.as_ref() {
        Some(r) => r,
        None => return Err(()),
    };

    // Parse path
    let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if components.is_empty() {
        return Err(());
    }

    // Traverse to parent of last component
    let mut current = root.clone();
    for i in 0..components.len() - 1 {
        let component = components[i];
        let children = current.children.lock();
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
    let device_name = components.last().unwrap();
    let entry = Arc::new(DevfsEntry::new_char_device_with_mode(device_name, devno, mode));

    current.children.lock().insert(String::from(*device_name), entry);

    Ok(())
}

/// Create directory
pub fn mkdir(path: &str) -> Result<(), ()> {
    // Remove leading /
    let path = path.strip_prefix('/').unwrap_or(path);

    if path.is_empty() {
        return Err(());
    }

    let root = DEVFS_ROOT.lock();
    let root = match root.as_ref() {
        Some(r) => r,
        None => return Err(()),
    };

    // Parse path
    let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    if components.is_empty() {
        return Err(());
    }

    // Traverse to parent of last component
    let mut current = root.clone();
    for i in 0..components.len() - 1 {
        let component = components[i];
        let children = current.children.lock();
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

    current.children.lock().insert(String::from(*dir_name), entry);

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
        let root = DEVFS_ROOT.lock();
        let root = root.as_ref()?;
        return Some((root.clone(), false, DevNo::default()));
    }

    let root = DEVFS_ROOT.lock();
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
        let children = current.children.lock();
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
    DEVFS_ROOT.lock().is_some()
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
    let root = DEVFS_ROOT.lock();
    let root = root.as_ref()?;

    // Empty path or "." means root directory
    if path.is_empty() || path == "/" || path == "." {
        let children = root.children.lock();
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
        let children = current.children.lock();
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
    let children = current.children.lock();
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
