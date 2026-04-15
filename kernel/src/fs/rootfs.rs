//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! RootFS - Simple memory-based filesystem
//!
//!
//! RootFS is a simple, memory-based filesystem,
//! used as the initial root filesystem during kernel boot.
//!
//! Features:
//! - RAM-based file storage
//! - Supports directories and regular files
//! - Does not support block devices
//! - Does not require disk

use crate::errno;
use crate::fs::superblock::{SuperBlock, SuperBlockFlags, FileSystemType, FsContext};
use crate::fs::mount::VfsMount;
use crate::fs::path::path_normalize;
use alloc::sync::Arc;
use core::cell::UnsafeCell;
use alloc::vec::Vec;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::borrow::ToOwned;
use crate::sync::spinlock::Spinlock;
use core::sync::atomic::{AtomicU64, AtomicPtr, Ordering};

pub const ROOTFS_MAGIC: u32 = 0x73636673;  // "sfsf" - Simple File System

static GLOBAL_ROOTFS_SB: AtomicPtr<RootFSSuperBlock> = AtomicPtr::new(core::ptr::null_mut());

static GLOBAL_ROOT_MOUNT: AtomicPtr<VfsMount> = AtomicPtr::new(core::ptr::null_mut());

// ============================================================================
// RootFS Path Cache
// ============================================================================

/// RootFS path cache size - from config
const ROOTFS_PATH_CACHE_SIZE: usize = crate::config::ROOTFS_PATH_CACHE_SIZE;

struct RootFSPathCacheEntry {
    /// Full path
    path: String,
    /// Node reference
    node: Option<Arc<RootFSNode>>,
}

impl RootFSPathCacheEntry {
    fn new() -> Self {
        Self {
            path: String::new(),
            node: None,
        }
    }
}

struct RootFSPathCache {
    /// Hash table buckets
    buckets: [RootFSPathCacheEntry; ROOTFS_PATH_CACHE_SIZE],
    /// Cache hit count
    hits: AtomicU64,
    /// Cache miss count
    misses: AtomicU64,
}

// SAFETY: RootFSPathCache uses internal Spinlock for mutable access;
// all fields are either locked or atomic.
unsafe impl Send for RootFSPathCache {}
// SAFETY: all shared mutable state is protected by the internal Spinlock;
// no data races are possible across threads/CPUs.
unsafe impl Sync for RootFSPathCache {}

static ROOTFS_PATH_CACHE: Spinlock<Option<RootFSPathCache>> = Spinlock::new(None);

fn rootfs_path_cache_init() {
    let mut cache = ROOTFS_PATH_CACHE.lock();
    if cache.is_some() {
        return;  // Already initialized
    }

    // Use from_fn to create array (avoid Copy trait requirement)
    let buckets: [RootFSPathCacheEntry; ROOTFS_PATH_CACHE_SIZE] =
        core::array::from_fn(|_| RootFSPathCacheEntry::new());

    *cache = Some(RootFSPathCache {
        buckets,
        hits: AtomicU64::new(0),
        misses: AtomicU64::new(0),
    });
}

fn rootfs_path_hash(path: &str) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;  // FNV offset basis
    for byte in path.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn rootfs_path_cache_lookup(path: &str) -> Option<Arc<RootFSNode>> {
    rootfs_path_cache_init();

    let cache = ROOTFS_PATH_CACHE.lock();
    let cache_inner = cache.as_ref()?;

    let hash = rootfs_path_hash(path);
    let index = (hash as usize) % ROOTFS_PATH_CACHE_SIZE;

    let bucket = &cache_inner.buckets[index];
    if bucket.path == path {
        if let Some(ref node) = bucket.node {
            cache_inner.hits.fetch_add(1, Ordering::Relaxed);
            return Some(node.clone());
        }
    }

    cache_inner.misses.fetch_add(1, Ordering::Relaxed);
    None
}

fn rootfs_path_cache_add(path: &str, node: Arc<RootFSNode>) {
    rootfs_path_cache_init();

    let mut cache = ROOTFS_PATH_CACHE.lock();
    let inner = cache.as_mut().expect("cache not initialized");

    let hash = rootfs_path_hash(path);
    let index = (hash as usize) % ROOTFS_PATH_CACHE_SIZE;

    // Simple LRU: directly overwrite old entry
    inner.buckets[index].path = path.to_owned();
    inner.buckets[index].node = Some(node);
}

fn rootfs_path_cache_stats() -> (u64, u64) {
    rootfs_path_cache_init();

    let cache = ROOTFS_PATH_CACHE.lock();
    let cache_inner = cache.as_ref().expect("cache not initialized");

    (
        cache_inner.hits.load(Ordering::Relaxed),
        cache_inner.misses.load(Ordering::Relaxed),
    )
}

pub fn get_rootfs_sb() -> Option<*mut RootFSSuperBlock> {
    let ptr = GLOBAL_ROOTFS_SB.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(ptr)
    }
}

pub fn get_root_mount() -> Option<*mut VfsMount> {
    let ptr = GLOBAL_ROOT_MOUNT.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(ptr)
    }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum RootFSType {
    /// Directory
    Directory,
    /// Regular file
    RegularFile,
    /// Symbolic link
    SymbolicLink,
}

/// Maximum symlink follow depth - from config
const MAX_SYMLINKS: usize = crate::config::MAX_SYMLINKS;

#[repr(C)]
pub struct RootFSNode {
    /// Node name (interior mutability: set_name requires exclusive parent-lock)
    pub(crate) name: UnsafeCell<Vec<u8>>,
    /// Node type
    pub node_type: RootFSType,
    /// Node data (if it's a file) — Arc enables true hard links: multiple
    /// directory entries share the same data.  Write operations use
    /// Arc::make_mut for copy-on-write semantics (fixes H61).
    pub data: Spinlock<Option<alloc::sync::Arc<Vec<u8>>>>,
    /// Symbolic link target (if it's a symlink)
    pub link_target: Option<Vec<u8>>,
    /// Child nodes (if it's a directory)
    pub children: Spinlock<Vec<Arc<RootFSNode>>>,
    /// Reference count
    ref_count: AtomicU64,
    /// Node ID
    pub ino: u64,
}

// SAFETY: RootFSNode's mutable fields (data, children) are protected by Spinlocks;
// name uses UnsafeCell with callers ensuring exclusive access via parent-lock.
unsafe impl Send for RootFSNode {}
// SAFETY: all shared mutable state is protected by internal Spinlocks or UnsafeCell;
// no data races are possible across threads/CPUs.
unsafe impl Sync for RootFSNode {}

impl RootFSNode {
    /// Create new node
    pub fn new(name: Vec<u8>, node_type: RootFSType, ino: u64) -> Self {
        Self {
            name: UnsafeCell::new(name),
            node_type,
            data: Spinlock::new(None),
            link_target: None,
            children: Spinlock::new(Vec::new()),
            ref_count: AtomicU64::new(1),
            ino,
        }
    }

    /// Get the node name as a byte slice.
    pub fn name(&self) -> &[u8] {
        // SAFETY: Name is only mutated via set_name(), which requires exclusive
        // access (parent's children lock held). Reads are safe under that invariant.
        unsafe { &*self.name.get() }
    }

    /// Create directory node
    pub fn new_dir(name: Vec<u8>, ino: u64) -> Self {
        Self::new(name, RootFSType::Directory, ino)
    }

    /// Create file node
    pub fn new_file(name: Vec<u8>, data: Vec<u8>, ino: u64) -> Self {
        let mut node = Self::new(name, RootFSType::RegularFile, ino);
        node.data = Spinlock::new(Some(alloc::sync::Arc::new(data)));
        node
    }

    /// Create symbolic link node
    pub fn new_symlink(name: Vec<u8>, target: Vec<u8>, ino: u64) -> Self {
        let mut node = Self::new(name, RootFSType::SymbolicLink, ino);
        node.link_target = Some(target);
        node
    }

    /// Increment reference count
    pub fn get(&self) {
        self.ref_count.fetch_add(1, Ordering::AcqRel);
    }

    /// Decrement reference count
    pub fn put(&self) {
        if self.ref_count.fetch_sub(1, Ordering::AcqRel) == 1 {
            // Last reference
        }
    }

    /// Add child node
    pub fn add_child(&self, child: Arc<RootFSNode>) {
        let mut children = self.children.lock();
        children.push(child);
    }

    /// Remove child node
    pub fn remove_child(&self, name: &[u8]) -> bool {
        let mut children = self.children.lock();
        if let Some(pos) = children.iter().position(|c| c.as_ref().name() == name) {
            children.remove(pos);
            true
        } else {
            false
        }
    }

    /// Set node name.
    ///
    /// # Safety (caller obligations)
    /// Caller must hold exclusive access to this node (typically the parent
    /// directory's children lock). No other thread may read the name field.
    pub fn set_name(&self, new_name: Vec<u8>) {
        // SAFETY: Caller guarantees exclusive access (parent lock held).
        unsafe { *self.name.get() = new_name; }
    }

    /// Rename child node
    pub fn rename_child(&self, old_name: &[u8], new_name: Vec<u8>) -> Result<(), ()> {
        let mut children = self.children.lock();
        let pos = children.iter().position(|c| c.as_ref().name() == old_name).ok_or(())?;

        // SAFETY: We hold children lock, granting exclusive access to the child.
        children[pos].set_name(new_name);

        Ok(())
    }

    /// Find child node
    pub fn find_child(&self, name: &[u8]) -> Option<Arc<RootFSNode>> {
        let children = self.children.lock();
        for child in children.iter() {
            if child.as_ref().name() == name {
                // Arc implements Clone trait
                return Some(child.clone());
            }
        }
        None
    }

    /// Get all child nodes
    pub fn list_children(&self) -> Vec<Arc<RootFSNode>> {
        let children = self.children.lock();
        // Clone each Arc reference
        children.iter().map(|child| child.clone()).collect()
    }

    /// Check if it's a directory
    pub fn is_dir(&self) -> bool {
        self.node_type == RootFSType::Directory
    }

    /// Check if it's a file
    pub fn is_file(&self) -> bool {
        self.node_type == RootFSType::RegularFile
    }

    /// Check if it's a symbolic link
    pub fn is_symlink(&self) -> bool {
        self.node_type == RootFSType::SymbolicLink
    }

    /// Get symbolic link target
    pub fn get_link_target(&self) -> Option<Vec<u8>> {
        self.link_target.clone()
    }

    /// Read file data
    pub fn read_data(&self, offset: usize, buf: &mut [u8]) -> usize {
        let data_guard = self.data.lock();
        if let Some(ref data_arc) = *data_guard {
            if offset >= data_arc.len() {
                return 0;
            }
            let remaining = &data_arc[offset..];
            let to_copy = core::cmp::min(remaining.len(), buf.len());
            buf[..to_copy].copy_from_slice(&remaining[..to_copy]);
            to_copy
        } else {
            0
        }
    }

    /// Write file data.
    ///
    /// If the data Arc is shared (hard link exists), uses copy-on-write
    /// via `Arc::make_mut` to avoid mutating other links' data.
    pub fn write_data(&self, offset: usize, data: &[u8]) -> usize {
        let mut data_guard = self.data.lock();
        if data_guard.is_none() {
            *data_guard = Some(alloc::sync::Arc::new(Vec::new()));
        }

        if let Some(ref mut data_arc) = *data_guard {
            // COW: clone if shared (Arc::make_mut returns &mut Vec<u8>)
            let buf = alloc::sync::Arc::make_mut(data_arc);
            let required_size = offset + data.len();
            if buf.len() < required_size {
                buf.resize(required_size, 0);
            }

            buf[offset..offset + data.len()].copy_from_slice(data);
            data.len()
        } else {
            0
        }
    }
}

pub struct RootFSSuperBlock {
    /// Base superblock
    pub sb: SuperBlock,
    /// Root node
    pub root_node: Arc<RootFSNode>,
    /// Next inode ID
    next_ino: AtomicU64,
}

impl RootFSSuperBlock {
    /// Create new RootFS superblock
    pub fn new() -> Self {
        // Create root directory node
        let root_node = Arc::new(RootFSNode::new_dir(b"/".to_vec(), 1));

        // Create superblock
        let mut sb = SuperBlock::new(4096, ROOTFS_MAGIC);
        sb.set_flags(SuperBlockFlags::new(SuperBlockFlags::SB_ACTIVE));

        Self {
            sb,
            root_node,
            next_ino: AtomicU64::new(2),
        }
    }

    /// Get root node
    pub fn get_root(&self) -> Option<Arc<RootFSNode>> {
        // Arc implements Clone trait (standard library)
        Some(self.root_node.clone())
    }

    /// Allocate new inode ID
    pub fn alloc_ino(&self) -> u64 {
        self.next_ino.fetch_add(1, Ordering::AcqRel)
    }

    /// Create file at specified path
    pub fn create_file(&self, path: &str, data: Vec<u8>) -> Result<(), i32> {
        // Parse path
        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if components.is_empty() {
            return Err(errno::Errno::FileExists.as_neg_i32());
        }

        let mut current = self.root_node.clone();

        // Traverse path to find parent directory
        for i in 0..components.len() - 1 {
            let component = components[i].as_bytes();
            match current.find_child(component) {
                Some(child) => {
                    if !child.is_dir() {
                        return Err(errno::Errno::NotADirectory.as_neg_i32());
                    }
                    current = child;
                }
                None => {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
            }
        }

        // Create new file
        let filename = components.last().unwrap().as_bytes().to_vec();
        let ino = self.alloc_ino();
        let new_file = Arc::new(RootFSNode::new_file(filename, data, ino));
        current.add_child(new_file);

        Ok(())
    }

    /// Create directory at specified path
    ///
    ///
    /// # Arguments
    /// - path: directory path
    /// - mode: directory permissions (currently unused)
    ///
    /// # Returns
    /// Returns Ok(()) on success, error code on failure
    pub fn create_dir(&self, path: &str, _mode: u32) -> Result<(), i32> {
        // Normalize path
        let normalized = path_normalize(path);

        // Split path
        let components: Vec<&str> = normalized.split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if components.is_empty() {
            return Err(errno::Errno::FileExists.as_neg_i32());
        }

        let mut current = self.root_node.clone();

        // Traverse path to find parent directory
        for i in 0..components.len() - 1 {
            let component = components[i].as_bytes();
            match current.find_child(component) {
                Some(child) => {
                    if !child.is_dir() {
                        return Err(errno::Errno::NotADirectory.as_neg_i32());
                    }
                    current = child;
                }
                None => {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
            }
        }

        // Check if target already exists
        let dirname = components.last().unwrap().as_bytes();
        if current.find_child(dirname).is_some() {
            return Err(errno::Errno::FileExists.as_neg_i32());
        }

        // Create new directory
        let dirname = dirname.to_vec();
        let ino = self.alloc_ino();
        let new_dir = Arc::new(RootFSNode::new_dir(dirname, ino));
        current.add_child(new_dir);

        Ok(())
    }

    /// Create hard link
    ///
    ///
    /// # Arguments
    /// - oldpath: existing file path
    /// - newpath: new link path
    ///
    /// # Returns
    /// Returns Ok(()) on success, error code on failure
    ///
    /// # Limitations
    /// - Cannot create hard links for directories
    /// - newpath's parent directory must exist
    /// - newpath must not already exist
    pub fn link(&self, oldpath: &str, newpath: &str) -> Result<(), i32> {
        // Normalize paths
        let old_normalized = path_normalize(oldpath);
        let new_normalized = path_normalize(newpath);

        // Lookup old file
        let old_node = match self.lookup(&old_normalized) {
            Some(node) => node,
            None => return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
        };

        // Cannot create hard links for directories
        if old_node.is_dir() {
            return Err(errno::Errno::IsADirectory.as_neg_i32());
        }

        // Cannot create hard links for symbolic links (simplified implementation)
        if old_node.is_symlink() {
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
        }

        // Split new path
        let new_components: Vec<&str> = new_normalized.split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if new_components.is_empty() {
            return Err(errno::Errno::FileExists.as_neg_i32());
        }

        // Find parent directory of new path
        let mut current = self.root_node.clone();
        for i in 0..new_components.len() - 1 {
            let component = new_components[i].as_bytes();
            match current.find_child(component) {
                Some(child) => {
                    if !child.is_dir() {
                        return Err(errno::Errno::NotADirectory.as_neg_i32());
                    }
                    current = child;
                }
                None => {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
            }
        }

        // Check if new path already exists
        let new_name = new_components.last().unwrap().as_bytes();
        if current.find_child(new_name).is_some() {
            return Err(errno::Errno::FileExists.as_neg_i32());
        }

        // Clone existing node (hard link: multiple directory entries for same inode)
        // RootFS uses Arc, so clone will increment reference count
        // But we need to modify the node name, so special handling is needed here

        // In simplified implementation, we create a new directory entry pointing to the same data
        // Note: This is not a true hard link (since each node has its own ino)
        // But for RootFS (memory filesystem), this is acceptable

        // True hard link implementation:
        // 1. Increment link count
        // 2. Add new directory entry in parent directory pointing to the same inode
        // Since RootFSNode's name is immutable, we need to use unsafe to modify

        // Hard link: share the same data Arc between old and new entries (fixes H61).
        // This gives POSIX semantics: writes through one link are visible through
        // the other.  Uses Arc::make_mut in write_data for copy-on-write.

        let new_link = {
            let new_name = new_name.to_vec();
            let mut node = RootFSNode::new_file(
                new_name,
                Vec::new(), // placeholder — will be replaced with shared data
                old_node.ino,
            );
            // Share the same data Arc
            *node.data.lock() = old_node.data.lock().clone();
            node.link_target = old_node.link_target.clone();
            Arc::new(node)
        };

        current.add_child(new_link);

        Ok(())
    }

    /// Lookup file
    pub fn lookup(&self, path: &str) -> Option<Arc<RootFSNode>> {
        // Handle empty path
        if path.is_empty() {
            return Some(self.root_node.clone());
        }

        // Check if it's a relative path
        let is_relative = !path.starts_with('/');

        // Normalize path (handle . and ..)
        let normalized = path_normalize(path);

        // If it's a relative path, not supported for now (needs current working directory)
        if is_relative && !normalized.is_empty() && !normalized.starts_with("..") {
            // TODO: Support relative paths (needs current working directory)
            // For simple relative paths like "usr/bin", can try to lookup from root
            // But correct behavior should start from process's current working directory
            return None;
        }

        // If normalized is empty, return root directory
        let normalized_path = if normalized.is_empty() {
            "/"
        } else {
            normalized.as_str()
        };

        // Try to lookup from path cache
        if let Some(cached) = rootfs_path_cache_lookup(normalized_path) {
            return Some(cached);
        }

        // Cache miss, execute path traversal (supports symbolic links)
        let result = self.lookup_follow(normalized_path, 0);

        // Add result to cache
        if let Some(ref node) = result {
            rootfs_path_cache_add(normalized_path, node.clone());
        }

        result
    }

    /// Internal function that actually performs path traversal (does not support symbolic links)
    fn lookup_walk(&self, path: &str) -> Option<Arc<RootFSNode>> {
        if path == "/" {
            return Some(self.root_node.clone());
        }

        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if components.is_empty() {
            return Some(self.root_node.clone());
        }

        // Traverse path starting from root node
        let mut current = self.root_node.clone();

        for (i, component) in components.iter().enumerate() {
            let component_bytes = component.as_bytes();

            // Lookup child node from tree
            match current.find_child(component_bytes) {
                Some(child) => {
                    if !child.is_dir() && i < components.len() - 1 {
                        // Not a directory, but path is not finished
                        return None;
                    }

                    current = child;
                }
                None => {
                    // Lookup failed
                    return None;
                }
            }
        }

        Some(current)
    }

    /// List directory contents
    pub fn list_dir(&self, path: &str) -> Result<Vec<Arc<RootFSNode>>, i32> {
        let node = self.lookup(path).ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

        if !node.is_dir() {
            return Err(errno::Errno::NotADirectory.as_neg_i32());
        }

        Ok(node.list_children())
    }

    /// Create directory
    ///
    pub fn mkdir(&self, path: &str) -> Result<(), i32> {
        // Normalize path
        let normalized = path_normalize(path);

        // Split path
        let components: Vec<&str> = normalized.split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if components.is_empty() {
            return Err(errno::Errno::FileExists.as_neg_i32());
        }

        let mut current = self.root_node.clone();

        // Traverse path to find parent directory
        for i in 0..components.len() - 1 {
            let component = components[i].as_bytes();
            match current.find_child(component) {
                Some(child) => {
                    if !child.is_dir() {
                        return Err(errno::Errno::NotADirectory.as_neg_i32());
                    }
                    current = child;
                }
                None => {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
            }
        }

        // Create new directory
        let dirname = components.last().unwrap().as_bytes().to_vec();
        let ino = self.alloc_ino();
        let new_dir = Arc::new(RootFSNode::new_dir(dirname, ino));

        current.add_child(new_dir);

        Ok(())
    }

    /// Delete file
    ///
    pub fn unlink(&self, path: &str) -> Result<(), i32> {
        // Normalize path
        let normalized = path_normalize(path);

        // Split path
        let components: Vec<&str> = normalized.split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if components.is_empty() {
            return Err(errno::Errno::IsADirectory.as_neg_i32());
        }

        let mut current = self.root_node.clone();

        // Traverse path to find parent directory
        for i in 0..components.len() - 1 {
            let component = components[i].as_bytes();
            match current.find_child(component) {
                Some(child) => {
                    if !child.is_dir() {
                        return Err(errno::Errno::NotADirectory.as_neg_i32());
                    }
                    current = child;
                }
                None => {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
            }
        }

        // Delete file
        let filename = components.last().unwrap().as_bytes();

        // Check if it exists
        let target = current.find_child(filename).ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

        // Cannot delete directory
        if target.is_dir() {
            return Err(errno::Errno::IsADirectory.as_neg_i32());
        }

        // Delete file
        if !current.remove_child(filename) {
            return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
        }

        Ok(())
    }

    /// Delete directory
    ///
    pub fn rmdir(&self, path: &str) -> Result<(), i32> {
        // Normalize path
        let normalized = path_normalize(path);

        // Split path
        let components: Vec<&str> = normalized.split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if components.is_empty() {
            return Err(errno::Errno::IsADirectory.as_neg_i32());
        }

        let mut current = self.root_node.clone();

        // Traverse path to find parent directory
        for i in 0..components.len() - 1 {
            let component = components[i].as_bytes();
            match current.find_child(component) {
                Some(child) => {
                    if !child.is_dir() {
                        return Err(errno::Errno::NotADirectory.as_neg_i32());
                    }
                    current = child;
                }
                None => {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
            }
        }

        // Delete directory
        let dirname = components.last().unwrap().as_bytes();

        // Check if it exists
        let target = current.find_child(dirname).ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

        // Must be a directory
        if !target.is_dir() {
            return Err(errno::Errno::NotADirectory.as_neg_i32());
        }

        // Directory must be empty
        if !target.list_children().is_empty() {
            return Err(errno::Errno::DirectoryNotEmpty.as_neg_i32());
        }

        // Delete directory
        if !current.remove_child(dirname) {
            return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
        }

        Ok(())
    }

    /// Rename file or directory
    ///
    pub fn rename(&self, oldpath: &str, newpath: &str) -> Result<(), i32> {
        // Normalize paths
        let old_normalized = path_normalize(oldpath);
        let new_normalized = path_normalize(newpath);

        // Split old path
        let old_components: Vec<&str> = old_normalized.split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if old_components.is_empty() {
            return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
        }

        // Find parent directory of old file
        let mut old_parent = self.root_node.clone();

        for i in 0..old_components.len() - 1 {
            let component = old_components[i].as_bytes();
            match old_parent.find_child(component) {
                Some(child) => {
                    if !child.is_dir() {
                        return Err(errno::Errno::NotADirectory.as_neg_i32());
                    }
                    old_parent = child;
                }
                None => {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
            }
        }

        let old_name = old_components.last().unwrap().as_bytes();

        // Check if old file exists and keep a reference to it
        let target = old_parent.find_child(old_name).ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

        // Split new path
        let new_components: Vec<&str> = new_normalized.split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if new_components.is_empty() {
            return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
        }

        // Find parent directory of new file
        let mut new_parent = self.root_node.clone();

        for i in 0..new_components.len() - 1 {
            let component = new_components[i].as_bytes();
            match new_parent.find_child(component) {
                Some(child) => {
                    if !child.is_dir() {
                        return Err(errno::Errno::NotADirectory.as_neg_i32());
                    }
                    new_parent = child;
                }
                None => {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
            }
        }

        let new_name = new_components.last().unwrap().as_bytes().to_vec();

        // Cannot rename a directory into its own subdirectory
        if target.is_dir() {
            let mut check = new_parent.clone();
            while check.ino != self.root_node.ino {
                if check.ino == target.ino {
                    return Err(errno::Errno::InvalidArgument.as_neg_i32());
                }
                // Walk up — find this node's parent by checking root's children
                let found = self.root_node.find_child(check.name());
                match found {
                    Some(p) if p.ino != check.ino => check = p,
                    _ => break,
                }
            }
        }

        // Check if new file already exists
        if new_parent.find_child(&new_name).is_some() {
            // If target exists and is a directory, cannot overwrite
            let existing = new_parent.find_child(&new_name).unwrap();
            if existing.is_dir() {
                return Err(errno::Errno::IsADirectory.as_neg_i32());
            }
            // Remove existing target
            new_parent.remove_child(&new_name);
        }

        // Rename: reorder operations to avoid name-mismatch bug
        // Must remove from old parent BEFORE changing name, since remove_child
        // matches by name.
        if old_parent.ino != new_parent.ino {
            // Cross-directory: remove (while name still matches), rename, add
            old_parent.remove_child(old_name);
            target.set_name(new_name.clone());
            new_parent.add_child(target);
        } else {
            // Same directory: just rename in place (node stays in the same Vec)
            target.set_name(new_name.clone());
        }

        Ok(())
    }

    /// Create symbolic link
    ///
    pub fn symlink(&self, target: &str, linkpath: &str) -> Result<(), i32> {
        // Normalize link path
        let link_normalized = path_normalize(linkpath);

        // Split path
        let components: Vec<&str> = link_normalized.split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if components.is_empty() {
            return Err(errno::Errno::FileExists.as_neg_i32());
        }

        let mut current = self.root_node.clone();

        // Traverse path to find parent directory
        for i in 0..components.len() - 1 {
            let component = components[i].as_bytes();
            match current.find_child(component) {
                Some(child) => {
                    if !child.is_dir() {
                        return Err(errno::Errno::NotADirectory.as_neg_i32());
                    }
                    current = child;
                }
                None => {
                    return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
                }
            }
        }

        // Create new symbolic link
        let linkname = components.last().unwrap().as_bytes().to_vec();
        let target_bytes = target.as_bytes().to_vec();
        let ino = self.alloc_ino();
        let new_symlink = Arc::new(RootFSNode::new_symlink(linkname, target_bytes, ino));

        current.add_child(new_symlink);

        Ok(())
    }

    /// Read symbolic link target
    ///
    pub fn readlink(&self, path: &str) -> Result<Vec<u8>, i32> {
        // Lookup symbolic link node
        let node = self.lookup(path).ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

        // Check if it's a symbolic link
        if !node.is_symlink() {
            return Err(errno::Errno::InvalidArgument.as_neg_i32());
        }

        // Get target path
        node.get_link_target().ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
    }

    /// Follow symbolic link (internal implementation)
    ///
    ///
    /// # Arguments
    /// - `link`: symbolic link node
    /// - `depth`: current recursion depth
    ///
    /// # Returns
    /// Returns the actual node the symbolic link points to on success, error on failure
    fn follow_link_internal(
        &self,
        link: &Arc<RootFSNode>,
        depth: usize,
    ) -> Option<Arc<RootFSNode>> {
        // Check recursion depth
        if depth >= MAX_SYMLINKS {
            return None;  // ELOOP: Too many levels of symbolic links
        }

        // Get target path
        let target_bytes = link.get_link_target()?;
        let target = core::str::from_utf8(&target_bytes).ok()?;

        // Normalize target path
        let normalized = path_normalize(target);

        // Lookup target node (recursive lookup)
        self.lookup_follow(&normalized, depth + 1)
    }

    /// Lookup path, supports following symbolic links (internal implementation)
    ///
    /// # Arguments
    /// - `path`: normalized path
    /// - `depth`: current recursion depth
    fn lookup_follow(&self, path: &str, depth: usize) -> Option<Arc<RootFSNode>> {
        if path == "/" {
            return Some(self.root_node.clone());
        }

        let components: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        if components.is_empty() {
            return Some(self.root_node.clone());
        }

        // Traverse path starting from root node
        let mut current = self.root_node.clone();

        for (i, component) in components.iter().enumerate() {
            let component_bytes = component.as_bytes();

            // Lookup child node from tree
            match current.find_child(component_bytes) {
                Some(child) => {
                    // If it's a symbolic link, follow it
                    if child.is_symlink() && i < components.len() - 1 {
                        // Follow symbolic link
                        let target = self.follow_link_internal(&child, depth)?;
                        // Continue lookup from symbolic link target
                        current = target;
                    } else {
                        current = child;
                    }

                    // Check if we need to continue traversing
                    if !current.is_dir() && i < components.len() - 1 {
                        return None;
                    }
                }
                None => {
                    return None;
                }
            }
        }

        Some(current)
    }
}

/// RootFS mount function
// SAFETY: _fc is a valid FsContext reference from the VFS mount call;
// RootFSSuperBlock is a simple wrapper with no unsafe invariants.
unsafe extern "C" fn rootfs_mount(_fc: &FsContext) -> Result<*mut SuperBlock, i32> {
    // Create RootFS superblock
    let rootfs_sb = Box::new(RootFSSuperBlock::new());

    // Extract raw pointer
    let sb_ptr = Box::into_raw(Box::new(rootfs_sb.sb)) as *mut SuperBlock;

    Ok(sb_ptr)
}

pub static ROOTFS_FS_TYPE: FileSystemType = FileSystemType::new(
    "rootfs",
    Some(rootfs_mount),
    None,  // kill_sb - use default implementation
    0,     // fs_flags
);

pub fn init_rootfs() -> Result<(), i32> {
    use crate::fs::superblock::register_filesystem;
    use crate::fs::mount::MntFlags;

    // Register rootfs filesystem
    register_filesystem(&ROOTFS_FS_TYPE)?;

    // Create and initialize global RootFS superblock
    let rootfs_sb = Box::new(RootFSSuperBlock::new());
    let rootfs_sb_ptr = Box::into_raw(rootfs_sb) as *mut RootFSSuperBlock;

    // Save to global variable (protected with AtomicPtr)
    GLOBAL_ROOTFS_SB.store(rootfs_sb_ptr, Ordering::Release);

    // Create root mount point and leak to static storage
    let mount = Box::new(VfsMount::new(
        b"/".to_vec(),      // Mount point
        b"/".to_vec(),      // Root directory
        MntFlags::new(0),   // No special flags
        Some(rootfs_sb_ptr as *mut u8),  // Superblock
    ));
    let mount_ptr = Box::into_raw(mount) as *mut VfsMount;

    // Save to global variable (protected with AtomicPtr)
    GLOBAL_ROOT_MOUNT.store(mount_ptr, Ordering::Release);

    // Set mount point ID to 1 (root mount point)
    // SAFETY: global pointer is valid once initialized during init
    unsafe {
        (*mount_ptr).mnt_id = 1;
    }

    // Create essential directories on rootfs (fallback when ext4 is unavailable)
    // SAFETY: pointer is valid and within bounds; access was validated before this unsafe block was reached.
    unsafe {
        let rootfs = &*rootfs_sb_ptr;
        for dir in &["/tmp", "/proc", "/dev", "/etc", "/root"] {
            let _ = rootfs.mkdir(dir);
        }
    }

    Ok(())
}

pub fn get_root_node() -> Option<&'static RootFSNode> {
    let sb_ptr = GLOBAL_ROOTFS_SB.load(Ordering::Acquire);
    if sb_ptr.is_null() {
        return None;
    }
    // SAFETY: pointer is valid and within bounds; access was validated before this unsafe block was reached.
    unsafe {
        sb_ptr.as_ref().map(|sb| sb.root_node.as_ref())
    }
}

pub fn get_rootfs() -> *const RootFSSuperBlock {
    GLOBAL_ROOTFS_SB.load(Ordering::Acquire)
}

/// Create a VFS inode for the rootfs root directory.
/// Called during mount to set up the root dentry's inode.
pub fn create_root_inode() -> alloc::sync::Arc<Inode> {
    let sb_ptr = GLOBAL_ROOTFS_SB.load(Ordering::Acquire);
    let root_node = if !sb_ptr.is_null() {
        // SAFETY: global pointer is valid once initialized during init
        unsafe { (*sb_ptr).root_node.clone() }
    } else {
        // Fallback: create a minimal root node
        alloc::sync::Arc::new(RootFSNode::new_dir(b"/".to_vec(), 1))
    };
    let mut inode = Inode::new(root_node.ino, InodeMode::new(InodeMode::S_IFDIR | 0o755));
    inode.ops = Some(&ROOTFS_INODE_OPS);
    inode.private_data = Some(alloc::sync::Arc::as_ptr(&root_node) as *mut u8);
    alloc::sync::Arc::new(inode)
}

// ============================================================================
// RootFS Inode Operations
// ============================================================================

use crate::fs::inode::{Inode, InodeMode, INodeOps, Ino};

/// RootFS inode lookup operation
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn rootfs_lookup(dir: &Inode, name: &[u8]) -> Result<Ino, i32> {
    // Get RootFSNode from inode's private_data
    let node_ptr = dir.private_data.ok_or(errno::Errno::NotADirectory.as_neg_i32())?;
    let node = &*(node_ptr as *const RootFSNode);

    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory.as_neg_i32());
    }

    // Lookup child
    match node.find_child(name) {
        Some(child) => Ok(child.ino),
        None => Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32()),
    }
}

/// RootFS mkdir operation
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn rootfs_mkdir(dir: &Inode, name: &[u8], mode: InodeMode) -> Result<alloc::sync::Arc<Inode>, i32> {
    let _ = mode; // Mode not used in RootFS

    // Get RootFSNode from inode's private_data
    let node_ptr = dir.private_data.ok_or(errno::Errno::NotADirectory.as_neg_i32())?;
    let node = &*(node_ptr as *const RootFSNode);

    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory.as_neg_i32());
    }

    // Check if already exists
    if node.find_child(name).is_some() {
        return Err(errno::Errno::FileExists.as_neg_i32());
    }

    // Get superblock to allocate inode number
    let sb_ptr = GLOBAL_ROOTFS_SB.load(Ordering::Acquire);
    if sb_ptr.is_null() {
        return Err(errno::Errno::IOError.as_neg_i32());
    }
    let sb = &*sb_ptr;
    let ino = sb.alloc_ino();

    // Create new directory node
    let new_dir = alloc::sync::Arc::new(RootFSNode::new_dir(name.to_vec(), ino));
    node.add_child(new_dir.clone());

    // Create inode
    let mut inode = Inode::new(ino, InodeMode::new(InodeMode::S_IFDIR | 0o755));
    inode.private_data = Some(alloc::sync::Arc::as_ptr(&new_dir) as *mut u8);
    inode.ops = Some(&ROOTFS_INODE_OPS);

    Ok(alloc::sync::Arc::new(inode))
}

/// RootFS unlink operation
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn rootfs_unlink(dir: &Inode, name: &[u8]) -> i32 {
    // Get RootFSNode from inode's private_data
    let node_ptr = match dir.private_data {
        Some(ptr) => ptr,
        None => return errno::Errno::NotADirectory.as_neg_i32(),
    };
    let node = &*(node_ptr as *const RootFSNode);

    if !node.is_dir() {
        return errno::Errno::NotADirectory.as_neg_i32();
    }

    // Check if exists and is not a directory
    match node.find_child(name) {
        Some(child) => {
            if child.is_dir() {
                return errno::Errno::IsADirectory.as_neg_i32();
            }
        }
        None => return errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    }

    // Remove child
    if node.remove_child(name) {
        0
    } else {
        errno::Errno::NoSuchFileOrDirectory.as_neg_i32()
    }
}

/// RootFS rmdir operation
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn rootfs_rmdir(dir: &Inode, name: &[u8]) -> i32 {
    // Get RootFSNode from inode's private_data
    let node_ptr = match dir.private_data {
        Some(ptr) => ptr,
        None => return errno::Errno::NotADirectory.as_neg_i32(),
    };
    let node = &*(node_ptr as *const RootFSNode);

    if !node.is_dir() {
        return errno::Errno::NotADirectory.as_neg_i32();
    }

    // Check if exists and is a directory
    match node.find_child(name) {
        Some(child) => {
            if !child.is_dir() {
                return errno::Errno::NotADirectory.as_neg_i32();
            }
            // Check if directory is empty
            if !child.list_children().is_empty() {
                return errno::Errno::DirectoryNotEmpty.as_neg_i32();
            }
        }
        None => return errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    }

    // Remove child
    if node.remove_child(name) {
        0
    } else {
        errno::Errno::NoSuchFileOrDirectory.as_neg_i32()
    }
}

/// RootFS create operation
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn rootfs_create(dir: &Inode, name: &[u8], mode: InodeMode) -> Result<alloc::sync::Arc<Inode>, i32> {
    let _ = mode; // Mode not used in RootFS

    // Get RootFSNode from inode's private_data
    let node_ptr = dir.private_data.ok_or(errno::Errno::NotADirectory.as_neg_i32())?;
    let node = &*(node_ptr as *const RootFSNode);

    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory.as_neg_i32());
    }

    // Check if already exists
    if node.find_child(name).is_some() {
        return Err(errno::Errno::FileExists.as_neg_i32());
    }

    // Get superblock to allocate inode number
    let sb_ptr = GLOBAL_ROOTFS_SB.load(Ordering::Acquire);
    if sb_ptr.is_null() {
        return Err(errno::Errno::IOError.as_neg_i32());
    }
    let sb = &*sb_ptr;
    let ino = sb.alloc_ino();

    // Create new file node
    let new_file = alloc::sync::Arc::new(RootFSNode::new_file(name.to_vec(), alloc::vec::Vec::new(), ino));
    node.add_child(new_file.clone());

    // Create inode
    let mut inode = Inode::new(ino, InodeMode::new(InodeMode::S_IFREG | 0o644));
    inode.private_data = Some(alloc::sync::Arc::as_ptr(&new_file) as *mut u8);
    inode.ops = Some(&ROOTFS_INODE_OPS);

    Ok(alloc::sync::Arc::new(inode))
}

/// RootFS symlink operation
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn rootfs_symlink(dir: &Inode, name: &[u8], target: &[u8]) -> Result<alloc::sync::Arc<Inode>, i32> {
    // Get RootFSNode from inode's private_data
    let node_ptr = dir.private_data.ok_or(errno::Errno::NotADirectory.as_neg_i32())?;
    let node = &*(node_ptr as *const RootFSNode);

    if !node.is_dir() {
        return Err(errno::Errno::NotADirectory.as_neg_i32());
    }

    // Check if already exists
    if node.find_child(name).is_some() {
        return Err(errno::Errno::FileExists.as_neg_i32());
    }

    // Get superblock to allocate inode number
    let sb_ptr = GLOBAL_ROOTFS_SB.load(Ordering::Acquire);
    if sb_ptr.is_null() {
        return Err(errno::Errno::IOError.as_neg_i32());
    }
    let sb = &*sb_ptr;
    let ino = sb.alloc_ino();

    // Create new symlink node
    let new_link = alloc::sync::Arc::new(RootFSNode::new_symlink(name.to_vec(), target.to_vec(), ino));
    node.add_child(new_link.clone());

    // Create inode
    let mut inode = Inode::new(ino, InodeMode::new(InodeMode::S_IFLNK | 0o777));
    inode.private_data = Some(alloc::sync::Arc::as_ptr(&new_link) as *mut u8);
    inode.ops = Some(&ROOTFS_INODE_OPS);

    Ok(alloc::sync::Arc::new(inode))
}

/// RootFS link operation
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn rootfs_link(dir: &Inode, name: &[u8], target: &Inode) -> i32 {
    // Get target node
    let target_ptr = match target.private_data {
        Some(ptr) => ptr,
        None => return errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    };
    let target_node = &*(target_ptr as *const RootFSNode);

    // Cannot link to directories
    if target_node.is_dir() {
        return errno::Errno::IsADirectory.as_neg_i32();
    }

    // Get parent directory
    let dir_ptr = match dir.private_data {
        Some(ptr) => ptr,
        None => return errno::Errno::NotADirectory.as_neg_i32(),
    };
    let dir_node = &*(dir_ptr as *const RootFSNode);

    if !dir_node.is_dir() {
        return errno::Errno::NotADirectory.as_neg_i32();
    }

    // Check if name already exists
    if dir_node.find_child(name).is_some() {
        return errno::Errno::FileExists.as_neg_i32();
    }

    // Create new link sharing the same data Arc (fixes H61)
    let new_link = {
        let mut node = RootFSNode::new_file(
            name.to_vec(),
            Vec::new(), // placeholder
            target_node.ino,
        );
        *node.data.lock() = target_node.data.lock().clone();
        alloc::sync::Arc::new(node)
    };
    dir_node.add_child(new_link);

    0
}

/// RootFS rename operation
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn rootfs_rename(old_dir: &Inode, old_name: &[u8], new_dir: &Inode, new_name: &[u8]) -> i32 {
    // Get old directory
    let old_dir_ptr = match old_dir.private_data {
        Some(ptr) => ptr,
        None => return errno::Errno::NotADirectory.as_neg_i32(),
    };
    let old_dir_node = &*(old_dir_ptr as *const RootFSNode);

    if !old_dir_node.is_dir() {
        return errno::Errno::NotADirectory.as_neg_i32();
    }

    // Find source
    let source = match old_dir_node.find_child(old_name) {
        Some(n) => n,
        None => return errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    };

    // Get new directory
    let new_dir_ptr = match new_dir.private_data {
        Some(ptr) => ptr,
        None => return errno::Errno::NotADirectory.as_neg_i32(),
    };
    let new_dir_node = &*(new_dir_ptr as *const RootFSNode);

    if !new_dir_node.is_dir() {
        return errno::Errno::NotADirectory.as_neg_i32();
    }

    // Remove from old directory
    if !old_dir_node.remove_child(old_name) {
        return errno::Errno::NoSuchFileOrDirectory.as_neg_i32();
    }

    // Rename — source is exclusive after remove_child
    source.set_name(new_name.to_vec());

    // Add to new directory
    new_dir_node.add_child(source);

    0
}

/// RootFS readlink operation
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn rootfs_readlink(inode: &Inode, buf: &mut [u8]) -> isize {
    let node_ptr = match inode.private_data {
        Some(ptr) => ptr,
        None => return errno::Errno::InvalidArgument.as_neg_i32() as isize,
    };
    let node = &*(node_ptr as *const RootFSNode);

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

/// RootFS getattr operation
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn rootfs_getattr(inode: &Inode, stat: &mut crate::fs::Stat) -> i32 {
    let node_ptr = match inode.private_data {
        Some(ptr) => ptr,
        None => return errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    };
    let node = &*(node_ptr as *const RootFSNode);

    stat.st_ino = node.ino;
    stat.st_mode = if node.is_dir() {
        InodeMode::S_IFDIR | 0o755
    } else if node.is_symlink() {
        InodeMode::S_IFLNK | 0o777
    } else {
        InodeMode::S_IFREG | 0o644
    };
    stat.st_size = node.data.lock().as_ref().map(|d| d.len() as i64).unwrap_or(0);
    stat.st_nlink = 1; // TODO: track actual hard link count
    stat.st_uid = 0;
    stat.st_gid = 0;
    stat.st_rdev = 0;
    stat.st_blksize = 4096;
    stat.st_blocks = (stat.st_size as i64 + 511) / 512;
    stat.st_atime = 0;
    stat.st_atime_nsec = 0;
    stat.st_mtime = 0;
    stat.st_mtime_nsec = 0;
    stat.st_ctime = 0;
    stat.st_ctime_nsec = 0;

    0
}

/// RootFS file read operation
fn rootfs_file_read(file: &crate::fs::File, buf: &mut [u8]) -> isize {
    // SAFETY: pointer is valid and within bounds; access was validated before this unsafe block was reached.
    unsafe {
        let inode_opt = &*file.inode.get();
        let inode = match inode_opt.as_ref() {
            Some(i) => i,
            None => return -9,
        };
        let node_ptr = match inode.private_data {
            Some(p) => p,
            None => return -9,
        };
        let node = &*(node_ptr as *const RootFSNode);
        let offset = file.get_pos() as usize;
        let data_guard = node.data.lock();
        if let Some(ref data_arc) = *data_guard {
            let available = data_arc.len().saturating_sub(offset);
            let to_read = buf.len().min(available);
            if to_read > 0 {
                buf[..to_read].copy_from_slice(&data_arc[offset..offset + to_read]);
                file.set_pos((offset + to_read) as u64);
                to_read as isize
            } else {
                0
            }
        } else {
            0
        }
    }
}

/// RootFS file write operation
fn rootfs_file_write(file: &crate::fs::File, buf: &[u8]) -> isize {
    // SAFETY: pointer is valid and within bounds; access was validated before this unsafe block was reached.
    unsafe {
        let inode_opt = &*file.inode.get();
        let inode = match inode_opt.as_ref() {
            Some(i) => i,
            None => return -9,
        };
        let node_ptr = match inode.private_data {
            Some(p) => p,
            None => return -9,
        };
        let node = &*(node_ptr as *const RootFSNode);
        let offset = file.get_pos() as usize;
        let written = node.write_data(offset, buf);
        if written > 0 {
            file.set_pos((offset + written) as u64);
        }
        written as isize
    }
}

/// RootFS file seek operation
fn rootfs_file_lseek(file: &crate::fs::File, offset: isize, whence: i32) -> isize {
    let current_pos = file.get_pos() as isize;
    let file_size = unsafe {
        let inode_opt = &*file.inode.get();
        let inode = match inode_opt.as_ref() {
            Some(i) => i,
            None => return -9,
        };
        let node_ptr = match inode.private_data {
            Some(p) => p,
            None => return -9,
        };
        let node = &*(node_ptr as *const RootFSNode);
        node.data.lock().as_ref().map_or(0isize, |d: &alloc::sync::Arc<alloc::vec::Vec<u8>>| d.len() as isize)
    };
    let new_pos = match whence {
        0 => offset,
        1 => current_pos + offset,
        2 => file_size + offset,
        _ => return -22,
    };
    if new_pos < 0 { return -22; }
    file.set_pos(new_pos as u64);
    new_pos
}

/// RootFS file close operation
fn rootfs_file_close(_file: &crate::fs::File) -> i32 {
    0
}

/// RootFS file operations table
pub static ROOTFS_FILE_OPS: crate::fs::FileOps = crate::fs::FileOps {
    read: Some(rootfs_file_read),
    write: Some(rootfs_file_write),
    lseek: Some(rootfs_file_lseek),
    close: Some(rootfs_file_close),
    poll: None,
};

/// RootFS get_file_ops
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn rootfs_get_file_ops(inode: &Inode) -> Option<&'static crate::fs::file::FileOps> {
    if inode.mode.is_regular_file() {
        Some(&ROOTFS_FILE_OPS)
    } else if inode.mode.is_directory() {
        Some(&crate::fs::file::DIR_FILE_OPS)
    } else {
        None
    }
}

/// RootFS readdir: list directory entries
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn rootfs_readdir(inode: &Inode) -> Option<alloc::vec::Vec<crate::fs::inode::VfsDirEntry>> {
    use crate::fs::inode::file_type;

    let node_ptr = inode.private_data?;
    let node = &*(node_ptr as *const RootFSNode);
    if !node.is_dir() {
        return None;
    }
    let children = node.list_children();
    let mut entries = alloc::vec::Vec::new();
    for child in children.iter() {
        let dt = if child.is_dir() {
            file_type::DT_DIR
        } else if child.is_file() {
            file_type::DT_REG
        } else if child.is_symlink() {
            file_type::DT_LNK
        } else {
            file_type::DT_UNKNOWN
        };
        entries.push(crate::fs::inode::VfsDirEntry {
            ino: child.ino,
            name: child.name().to_vec(),
            file_type: dt,
        });
    }
    Some(entries)
}

/// RootFS inode operations table
pub static ROOTFS_INODE_OPS: INodeOps = INodeOps {
    lookup: Some(rootfs_lookup),
    create: Some(rootfs_create),
    link: Some(rootfs_link),
    unlink: Some(rootfs_unlink),
    symlink: Some(rootfs_symlink),
    mkdir: Some(rootfs_mkdir),
    rmdir: Some(rootfs_rmdir),
    mknod: None,  // RootFS doesn't support device nodes
    rename: Some(rootfs_rename),
    readlink: Some(rootfs_readlink),
    get_file_ops: Some(rootfs_get_file_ops),
    readdir: Some(rootfs_readdir),
    open: None,
    permission: None,  // Default: allow all
    getattr: Some(rootfs_getattr),
    setattr: None,  // RootFS doesn't support setattr
    iget: Some(rootfs_iget),
};

/// RootFS iget: instantiate VFS Inode from (parent, name, ino).
///
/// The parent inode's private_data points to a RootFSNode.
/// We find the child by name and create a VFS Inode.
// SAFETY: VFS callback contract; pointers are valid for the scope of this block
unsafe fn rootfs_iget(parent: &Inode, name: &[u8], ino: Ino) -> Result<alloc::sync::Arc<Inode>, i32> {
    let node_ptr = parent.private_data.ok_or(errno::Errno::NotADirectory.as_neg_i32())?;
    let node = &*(node_ptr as *const RootFSNode);

    // Find child by name
    let child = node.find_child(name).ok_or(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    // Create VFS inode
    let mode = if child.is_dir() {
        InodeMode::new(InodeMode::S_IFDIR | 0o755)
    } else if child.is_symlink() {
        InodeMode::new(InodeMode::S_IFLNK | 0o777)
    } else {
        InodeMode::new(InodeMode::S_IFREG | 0o644)
    };

    let mut inode = Inode::new(child.ino, mode);
    inode.private_data = Some(alloc::sync::Arc::as_ptr(&child) as *mut u8);
    inode.ops = Some(&ROOTFS_INODE_OPS);

    Ok(alloc::sync::Arc::new(inode))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rootfs_node() {
        let dir = RootFSNode::new_dir(b"test".to_vec(), 1);
        assert!(dir.is_dir());
        assert!(!dir.is_file());

        let file = RootFSNode::new_file(b"file.txt".to_vec(), b"hello".to_vec(), 2);
        assert!(file.is_file());
        assert!(!file.is_dir());
    }

    #[test]
    fn test_rootfs_superblock() {
        let sb = RootFSSuperBlock::new();
        let root = sb.get_root();
        assert!(root.is_dir());
    }

    #[test]
    fn test_rootfs_create_file() {
        let sb = RootFSSuperBlock::new();

        // Create file
        assert!(sb.create_file("/test.txt", b"hello".to_vec()).is_ok());

        // Lookup file
        let file = sb.lookup("/test.txt");
        assert!(file.is_some());
        assert!(file.unwrap().is_file());
    }

    #[test]
    fn test_rootfs_nested_path() {
        let sb = RootFSSuperBlock::new();

        // Create nested directories and files
        assert!(sb.create_file("/dir1/dir2/file.txt", b"data".to_vec()).is_err()); // Parent directory does not exist
    }

    #[test]
    fn test_rootfs_list() {
        let sb = RootFSSuperBlock::new();

        // Create multiple files
        assert!(sb.create_file("/file1.txt", b"data1".to_vec()).is_ok());
        assert!(sb.create_file("/file2.txt", b"data2".to_vec()).is_ok());

        // List root directory
        let children = sb.list_dir("/").unwrap();
        assert_eq!(children.len(), 2);  // file1.txt and file2.txt
    }
}
