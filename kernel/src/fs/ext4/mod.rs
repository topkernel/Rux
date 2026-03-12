//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! ext4 filesystem
//!
//!
//! Core concepts:
//! - `struct ext4_super_block`: ext4 superblock
//! - `struct ext4_inode`: ext4 inode
//! - `struct ext4_group_desc`: block group descriptor
//! - `struct ext4_dir_entry`: directory entry
//!
//! Reference: Documentation/filesystems/ext4/

pub mod superblock;
pub mod inode;
pub mod dir;
pub mod file;
pub mod allocator;
pub mod indirect;
pub mod extent;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::errno;
use crate::drivers::blkdev;
use crate::fs::bio;
use crate::fs::superblock::{FileSystemType, FsContext, SuperBlock};

pub const EXT4_SUPER_MAGIC: u16 = 0xEF53;

pub struct Ext4FileSystem {
    /// Block device
    pub device: *const blkdev::GenDisk,
    /// Superblock information
    pub sb_info: Option<Box<superblock::Ext4SuperBlockInfo>>,
    /// Block group descriptor table
    pub group_descs: Vec<Box<superblock::Ext4GroupDesc>>,
    /// Block size
    pub block_size: u32,
    /// Block size bits
    pub block_size_bits: u8,
    /// Inode size
    pub inode_size: u16,
    /// Blocks per group
    pub blocks_per_group: u32,
    /// Inodes per group
    pub inodes_per_group: u32,
    /// Number of block groups
    pub group_count: u32,
    /// Total blocks
    pub total_blocks: u64,
    /// Total inodes
    pub total_inodes: u32,
}

unsafe impl Send for Ext4FileSystem {}
unsafe impl Sync for Ext4FileSystem {}

impl Ext4FileSystem {
    /// Create new ext4 filesystem instance
    pub fn new(device: *const blkdev::GenDisk) -> Self {
        Self {
            device,
            sb_info: None,
            group_descs: Vec::new(),
            block_size: 4096,
            block_size_bits: 12,
            inode_size: 256,
            blocks_per_group: 0,
            inodes_per_group: 0,
            group_count: 0,
            total_blocks: 0,
            total_inodes: 0,
        }
    }

    /// Initialize ext4 filesystem
    ///
    /// Read superblock and block group descriptors
    pub fn init(&mut self) -> Result<(), i32> {
        unsafe {
            // Read superblock
            // ext4 superblock is at byte offset 1024
            // - For 1KB blocks: superblock at start of block 1
            // - For 2KB+ blocks: superblock at offset 1024 within block 0
            // Since we use 4KB block cache, read block 0 and access offset 1024
            let sb_bh = bio::bread(self.device, 0)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let sb_data = &(*sb_bh).b_data;
            // Superblock is at 1024 byte offset within block
            let ext4_sb = &*(sb_data.as_ptr().add(1024) as *const superblock::Ext4SuperBlockOnDisk);

            // Verify magic number
            if ext4_sb.s_magic != EXT4_SUPER_MAGIC {
                bio::brelse(sb_bh);
                return Err(errno::Errno::IOError.as_neg_i32());
            }

            // Parse superblock
            let block_size = 1024 << ext4_sb.s_log_block_size;
            let block_size_bits = (12 + ext4_sb.s_log_block_size) as u8;
            let blocks_per_group = ext4_sb.s_blocks_per_group;
            let inodes_per_group = ext4_sb.s_inodes_per_group;
            let total_blocks = ext4_sb.s_blocks_count;
            let total_inodes = ext4_sb.s_inodes_count;
            let group_count = ((total_blocks as u64) + (blocks_per_group as u64) - 1) /
                (blocks_per_group as u64);

            // Read block group descriptor table
            // Block group descriptor table starts at block (block_size / 1024) + 1
            let gd_start_block = if block_size == 1024 { 2 } else { 1 };
            let gds_per_block = block_size / core::mem::size_of::<superblock::Ext4GroupDesc>() as u32;
            let _gd_blocks = (group_count as u32 + gds_per_block - 1) / gds_per_block;

            let mut group_descs = Vec::new();

            for i in 0..group_count {
                let gd_block = gd_start_block + (i as u32 / gds_per_block);
                let gd_offset = (i as u32 % gds_per_block) as usize;

                let gd_bh = bio::bread(self.device, gd_block as u64)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;

                let gd_data = &(*gd_bh).b_data;
                let gd_ptr = unsafe {
                    &*(gd_data.as_ptr().add(gd_offset * core::mem::size_of::<superblock::Ext4GroupDesc>())
                        as *const superblock::Ext4GroupDesc)
                };

                group_descs.push(Box::new(*gd_ptr));
                bio::brelse(gd_bh);
            }

            bio::brelse(sb_bh);

            // Update filesystem information
            self.sb_info = Some(Box::new(superblock::Ext4SuperBlockInfo {
                s_inodes_count: ext4_sb.s_inodes_count,
                s_blocks_count: ext4_sb.s_blocks_count as u64,
                s_r_blocks_count: ext4_sb.s_r_blocks_count as u64,
                s_free_blocks_count: ext4_sb.s_free_blocks_count as u64,
                s_free_inodes_count: ext4_sb.s_free_inodes_count,
                s_first_data_block: ext4_sb.s_first_data_block,
                s_log_block_size: ext4_sb.s_log_block_size,
                s_blocks_per_group: ext4_sb.s_blocks_per_group,
                s_inodes_per_group: ext4_sb.s_inodes_per_group,
            }));

            self.block_size = block_size;
            self.block_size_bits = block_size_bits;
            self.inode_size = ext4_sb.s_inode_size;
            self.blocks_per_group = blocks_per_group;
            self.inodes_per_group = inodes_per_group;
            self.group_count = group_count as u32;
            self.total_blocks = total_blocks as u64;
            self.total_inodes = total_inodes;
            self.group_descs = group_descs;

            Ok(())
        }
    }

    /// Read inode
    pub fn read_inode(&self, ino: u32) -> Result<inode::Ext4Inode, i32> {
        unsafe {
            // Calculate block group and inode table index
            let group = (ino - 1) / self.inodes_per_group;
            let index = (ino - 1) % self.inodes_per_group;

            if group as usize >= self.group_descs.len() {
                return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
            }

            let gd = &self.group_descs[group as usize];

            // Calculate inode block number
            let inode_table_start = gd.bg_inode_table;
            let inodes_per_block = self.block_size / (self.inode_size as u32);
            let inode_block = inode_table_start + (index / inodes_per_block);
            let inode_offset = ((index % inodes_per_block) * (self.inode_size as u32)) as usize;

            // Read block containing inode
            let bh = bio::bread(self.device, inode_block as u64)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;

            let data = &(*bh).b_data;

            // Parse inode
            let ext4_inode = &*(data.as_ptr().add(inode_offset) as *const inode::Ext4InodeOnDisk);

            let result = inode::Ext4Inode::from_disk(ext4_inode, ino);

            bio::brelse(bh);
            Ok(result)
        }
    }

    /// Get root inode
    pub fn get_root_inode(&self) -> Result<inode::Ext4Inode, i32> {
        // Root inode number in ext4 is always 2
        self.read_inode(2)
    }

    /// Lookup directory entry
    pub fn lookup(&self, dir: &inode::Ext4Inode, name: &str) -> Result<dir::Ext4DirEntry, i32> {
        unsafe {
            // Traverse directory's data blocks
            let blocks = dir.get_data_blocks(self)?;
            let _name_bytes = name.as_bytes();

            for block in blocks {
                let bh = bio::bread(self.device, block)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;

                let data = &(*bh).b_data;
                let mut offset = 0;

                while offset < self.block_size as usize {
                    let entry = dir::Ext4DirEntry::from_bytes(
                        &data[offset..],
                        self.block_size as usize,
                    );

                    if entry.inode == 0 {
                        offset += entry.rec_len as usize;
                        continue;
                    }

                    let entry_name = core::str::from_utf8_unchecked(&entry.name[..entry.name_len as usize]);

                    if entry_name == name {
                        bio::brelse(bh);
                        return Ok(entry);
                    }

                    offset += entry.rec_len as usize;
                }

                bio::brelse(bh);
            }

            Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32())
        }
    }

    /// List directory contents
    ///
    /// # Arguments
    /// - `dir`: Directory inode
    ///
    /// # Returns
    /// List of directory entries
    pub fn list_dir(&self, dir: &inode::Ext4Inode) -> Result<Vec<dir::Ext4DirEntry>, i32> {
        unsafe {
            let mut entries = Vec::new();

            // Traverse directory's data blocks
            let blocks = dir.get_data_blocks(self)?;

            for block in blocks {
                let bh = bio::bread(self.device, block)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;

                let data = &(*bh).b_data;
                let mut offset = 0;

                while offset < self.block_size as usize {
                    let entry = dir::Ext4DirEntry::from_bytes(
                        &data[offset..],
                        self.block_size as usize,
                    );

                    if entry.inode == 0 {
                        offset += entry.rec_len as usize;
                        continue;
                    }

                    // Skip . and ..
                    let name = core::str::from_utf8_unchecked(&entry.name[..entry.name_len as usize]);
                    if name != "." && name != ".." {
                        entries.push(entry.clone());
                    }

                    offset += entry.rec_len as usize;
                }

                bio::brelse(bh);
            }

            Ok(entries)
        }
    }

    /// Read symbolic link target path
    ///
    /// # Arguments
    /// - `inode`: Symbolic link inode
    ///
    /// # Returns
    /// Symbolic link target path
    fn read_symlink_target(&self, inode: &inode::Ext4Inode) -> Result<String, i32> {
        let size = inode.get_size() as usize;

        // Fast symlink: target stored in block array (<= 60 bytes)
        if size <= 60 {
            let bytes = unsafe {
                core::slice::from_raw_parts(
                    &inode.block[0] as *const _ as *const u8,
                    size
                )
            };
            return Ok(String::from_utf8_lossy(bytes).into_owned());
        }

        // Slow symlink: target stored in data blocks
        let blocks = inode.get_data_blocks(self)?;
        let mut target = String::new();

        for block in blocks {
            unsafe {
                let bh = bio::bread(self.device, block)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;
                let data = &(*bh).b_data;

                let remaining = size - target.len();
                let to_read = core::cmp::min(remaining, self.block_size as usize);

                let bytes = &data[..to_read];
                target.push_str(&String::from_utf8_lossy(bytes));

                bio::brelse(bh);

                if target.len() >= size {
                    break;
                }
            }
        }

        Ok(target)
    }

    /// Lookup inode by path (following symbolic links)
    ///
    /// # Arguments
    /// - `path`: File path (absolute path, e.g. "/bin/sh")
    ///
    /// # Returns
    /// Inode number and inode structure
    pub fn lookup_path(&self, path: &str) -> Result<(u32, inode::Ext4Inode), i32> {
        self.lookup_path_internal(path, 0)
    }

    /// Internal path lookup implementation (with symlink depth limit to prevent loops)
    fn lookup_path_internal(&self, path: &str, symlink_depth: u32) -> Result<(u32, inode::Ext4Inode), i32> {
        const MAX_SYMLINK_DEPTH: u32 = 8;

        if symlink_depth > MAX_SYMLINK_DEPTH {
            return Err(errno::Errno::TooManySymbolicLinks.as_neg_i32());
        }

        // Parse path
        let path_parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        // Start from root inode
        let mut current_inode = self.get_root_inode()?;
        let mut current_ino = 2u32; // Root inode number

        // Traverse path
        for (idx, part) in path_parts.iter().enumerate() {
            let entry = self.lookup(&current_inode, *part)?;

            // Read next level inode
            current_ino = entry.inode;
            current_inode = self.read_inode(entry.inode)?;

            // If it's a symbolic link, follow it
            if current_inode.is_symlink() {
                let target = self.read_symlink_target(&current_inode)?;

                // Build remaining path
                let remaining: Vec<&str> = path_parts[idx + 1..].to_vec();

                // Build full target path
                let full_target = if target.starts_with('/') {
                    // Absolute path
                    if remaining.is_empty() {
                        target
                    } else {
                        let mut t = target;
                        for r in remaining {
                            t.push('/');
                            t.push_str(r);
                        }
                        t
                    }
                } else {
                    // Relative path - relative to current directory
                    let mut base_parts: Vec<&str> = path_parts[..idx].to_vec();
                    let target_parts: Vec<&str> = target.split('/').filter(|s| !s.is_empty()).collect();

                    for tp in target_parts {
                        if tp == ".." {
                            base_parts.pop();
                        } else if tp != "." {
                            base_parts.push(tp);
                        }
                    }

                    // Add remaining path
                    for r in remaining {
                        base_parts.push(r);
                    }

                    let mut result = String::new();
                    for p in base_parts {
                        result.push('/');
                        result.push_str(p);
                    }
                    if result.is_empty() {
                        result.push('/');
                    }
                    result
                };

                // Recursively lookup target path
                return self.lookup_path_internal(&full_target, symlink_depth + 1);
            }
        }

        Ok((current_ino, current_inode))
    }
}

static EXT4_FS_TYPE: FileSystemType = FileSystemType::new(
    "ext4",
    Some(ext4_mount),
    Some(ext4_kill_sb),
    0,
);

unsafe extern "C" fn ext4_mount(fc: &FsContext) -> Result<*mut SuperBlock, i32> {
    use crate::console::putchar;

    const MSG: &[u8] = b"ext4: mounting...\n";
    for &b in MSG {
        putchar(b);
    }

    // Get source device
    let _source = fc.source.ok_or(-2_i32)?;  // ENOENT

    // TODO: Get block device from source
    // Simplified implementation: assume device is already registered
    // Need to implement device name to device mapping

    // Create ext4 filesystem instance
    let mut fs = Box::new(Ext4FileSystem::new(core::ptr::null()));

    // Initialize filesystem
    fs.init()?;

    // Create VFS superblock
    let mut sb = Box::new(SuperBlock::new(fs.block_size as usize, EXT4_SUPER_MAGIC as u32));
    sb.set_type(&EXT4_FS_TYPE);
    sb.set_flags(crate::fs::superblock::SuperBlockFlags::new(
        crate::fs::superblock::SuperBlockFlags::SB_RDONLY,
    ));

    // Set private data
    let fs_ptr = Box::into_raw(fs) as *mut u8;
    sb.set_fs_info(fs_ptr);

    Ok(Box::into_raw(sb) as *mut SuperBlock)
}

unsafe extern "C" fn ext4_kill_sb(sb: *mut SuperBlock) {
    if let Some(fs_info) = (*sb).s_fs_info {
        let _fs = Box::from_raw(fs_info as *mut Ext4FileSystem);
        // Box will be automatically freed
    }

    let _sb = Box::from_raw(sb);
    // Box will be automatically freed
}

/// Read entire file from ext4 filesystem (supports symbolic links)
///
/// # Parameters
/// - `device`: Block device pointer
/// - `path`: File path (absolute path, e.g. "/bin/sh")
///
/// # Returns
/// - `Some(data)`: File content
/// - `None`: Read failed
pub fn read_file(device: *const blkdev::GenDisk, path: &str) -> Option<Vec<u8>> {
    read_file_internal(device, path, 0)
}

/// Internal implementation, supports recursion depth limit to prevent circular symbolic links
fn read_file_internal(device: *const blkdev::GenDisk, path: &str, depth: u32) -> Option<Vec<u8>> {
    use alloc::vec::Vec;

    // Prevent circular symbolic links, max recursion depth 8
    if depth > 8 {
        return None;
    }

    unsafe {
        // Create ext4 filesystem instance
        let mut fs = Box::new(Ext4FileSystem::new(device));

        // Initialize filesystem
        if fs.init().is_err() {
            return None;
        }

        // Parse path
        let path_parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

        // Start from root inode
        let mut current_inode = match fs.get_root_inode() {
            Ok(inode) => inode,
            Err(_) => {
                return None;
            }
        };

        // Record current path directory part (for resolving relative path symbolic links)
        let mut current_dir_parts: Vec<&str> = Vec::new();

        // Traverse path
        for part in path_parts.iter() {
            let entry = match fs.lookup(&current_inode, part) {
                Ok(e) => e,
                Err(_) => {
                    return None;
                }
            };

            // Read target inode
            let target_inode = match fs.read_inode(entry.inode) {
                Ok(inode) => inode,
                Err(_) => {
                    return None;
                }
            };

            // Check if it's a symbolic link
            if target_inode.is_symlink() {
                // Read symbolic link target
                let link_target = read_symlink_target(&fs, &target_inode)?;

                // Resolve target path
                let resolved_path = if link_target.starts_with('/') {
                    // Absolute path
                    link_target
                } else {
                    // Relative path, resolve based on current directory
                    let mut resolved = String::from("/");
                    for dir_part in &current_dir_parts {
                        resolved.push_str(dir_part);
                        resolved.push('/');
                    }
                    resolved.push_str(&link_target);
                    resolved
                };

                // Recursively read target file
                return read_file_internal(device, &resolved_path, depth + 1);
            }

            // If it's a directory, update current directory path
            if target_inode.is_dir() {
                current_dir_parts.push(part);
            }

            current_inode = target_inode;
        }

        // Read file content
        let file_size = current_inode.get_size() as usize;
        if file_size == 0 {
            return Some(Vec::new());
        }

        let mut buffer = Vec::with_capacity(file_size);
        buffer.resize(file_size, 0);

        match file::ext4_file_read(&fs, &current_inode, 0, &mut buffer) {
            Ok(n) => {
                buffer.truncate(n);
                Some(buffer)
            }
            Err(_) => None,
        }
    }
}

/// Read symbolic link target
///
/// ext4 symbolic link target storage methods:
/// - Short links (< 60 bytes): stored in inode's block array
/// - Long links: stored in data blocks
fn read_symlink_target(fs: &Ext4FileSystem, inode: &inode::Ext4Inode) -> Option<String> {
    let size = inode.get_size() as usize;
    if size == 0 || size > 4096 {
        return None;
    }

    let mut buffer = alloc::vec![0u8; size];

    // Short symbolic link: data stored in block array (inline data)
    // ext4 short symbolic link threshold is usually 60 bytes
    if size < 60 && !inode.has_extent() {
        // Read directly from block array
        let block_data = unsafe {
            core::slice::from_raw_parts(inode.block.as_ptr() as *const u8, 60)
        };
        buffer[..size].copy_from_slice(&block_data[..size]);
    } else {
        // Long symbolic link: read from data block
        match inode.read_data(fs, 0, &mut buffer) {
            Ok(n) if n == size => {}
            _ => return None,
        }
    }

    // Convert to string
    String::from_utf8(buffer).ok()
}

pub fn init() {
    use crate::console::putchar;

    // Register filesystem type
    let _ = crate::fs::superblock::register_filesystem(&EXT4_FS_TYPE);
}

/// Global ext4 filesystem instance
static GLOBAL_EXT4_FS: core::sync::atomic::AtomicPtr<Ext4FileSystem> =
    core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

/// Mount ext4 filesystem
///
/// # Parameters
/// - `device`: Block device pointer
///
/// # Returns
/// - `Ok(())`: Mount successful
/// - `Err(code)`: Mount failed
pub fn mount_ext4(device: *const blkdev::GenDisk) -> Result<(), i32> {
    use core::sync::atomic::Ordering;

    if device.is_null() {
        return Err(-22); // EINVAL
    }

    // Create ext4 filesystem instance
    let mut fs = Box::new(Ext4FileSystem::new(device));

    // Initialize filesystem
    fs.init()?;

    // Save to global variable
    let fs_ptr = Box::into_raw(fs);
    GLOBAL_EXT4_FS.store(fs_ptr, Ordering::Release);

    Ok(())
}

/// Get mounted ext4 filesystem
pub fn get_ext4_fs() -> Option<*mut Ext4FileSystem> {
    use core::sync::atomic::Ordering;
    let ptr = GLOBAL_EXT4_FS.load(Ordering::Acquire);
    if ptr.is_null() {
        None
    } else {
        Some(ptr)
    }
}

/// List directory contents from mounted ext4
///
/// # Parameters
/// - `path`: Directory path (absolute or relative path, e.g. "/bin" or ".")
///
/// # Returns
/// - `Some(entries)`: Directory entry list
/// - `None`: Read failed or directory doesn't exist
pub fn list_dir(path: &str) -> Option<Vec<dir::Ext4DirEntry>> {
    use core::sync::atomic::Ordering;

    let fs_ptr = GLOBAL_EXT4_FS.load(Ordering::Acquire);
    if fs_ptr.is_null() {
        return None;
    }

    // Parse path to absolute path
    let abs_path = resolve_path(path);

    unsafe {
        let fs = &*fs_ptr;

        // Find directory inode
        let (_, dir_inode) = fs.lookup_path(&abs_path).ok()?;

        // List directory contents
        fs.list_dir(&dir_inode).ok()
    }
}

/// Resolve path to absolute path
/// Supports relative paths and current working directory
fn resolve_path(path: &str) -> String {
    // If absolute path, return directly
    if path.starts_with('/') {
        return String::from(path);
    }

    // Get current working directory
    let cwd = if let Some(current) = crate::sched::current() {
        let cwd_bytes = unsafe { (*current).get_cwd() };
        match core::str::from_utf8(cwd_bytes) {
            Ok(s) => String::from(s),
            Err(_) => String::from("/"),
        }
    } else {
        String::from("/")
    };

    // Build full path
    let mut full_path = String::new();
    full_path.push_str(&cwd);
    if !cwd.ends_with('/') {
        full_path.push('/');
    }
    full_path.push_str(path);

    // Handle . and ..
    normalize_path(&full_path)
}

/// Normalize path (handle . and ..)
fn normalize_path(path: &str) -> String {
    let mut components: Vec<&str> = Vec::new();

    for part in path.split('/') {
        match part {
            "" | "." => {
                // Ignore empty parts and current directory
            }
            ".." => {
                // Go up one directory level
                components.pop();
            }
            _ => {
                components.push(part);
            }
        }
    }

    if components.is_empty() {
        String::from("/")
    } else {
        let mut result = String::new();
        for part in components {
            result.push('/');
            result.push_str(part);
        }
        result
    }
}

/// Check if ext4 is mounted
pub fn is_mounted() -> bool {
    use core::sync::atomic::Ordering;
    !GLOBAL_EXT4_FS.load(Ordering::Acquire).is_null()
}

/// Read file from mounted ext4 filesystem
///
/// # Parameters
/// - `path`: File path (absolute path)
///
/// # Returns
/// - `Some(data)`: File content
/// - `None`: Read failed
pub fn read_file_from_mounted(path: &str) -> Option<alloc::vec::Vec<u8>> {
    use core::sync::atomic::Ordering;

    let fs_ptr = GLOBAL_EXT4_FS.load(Ordering::Acquire);
    if fs_ptr.is_null() {
        return None;
    }

    // Parse path to absolute path
    let abs_path = resolve_path(path);

    unsafe {
        let fs = &*fs_ptr;
        let device = fs.device;

        // Use existing read_file function
        read_file(device, &abs_path)
    }
}
