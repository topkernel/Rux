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
pub mod namei;
pub mod journal;

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
    /// Block group descriptor table (Mutex-protected for safe concurrent access)
    pub group_descs: spin::Mutex<Vec<Box<superblock::Ext4GroupDesc>>>,
    /// Block size
    pub block_size: u32,
    /// Block size bits
    pub block_size_bits: u8,
    /// Group descriptor size (32 or 64 bytes depending on 64-bit feature)
    pub desc_size: u16,
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
    /// Journal inode number (typically 8)
    pub journal_ino: u32,
    /// JBD2 journal (initialized during mount)
    pub journal: Option<alloc::sync::Arc<crate::fs::jbd2::Journal>>,
}

unsafe impl Send for Ext4FileSystem {}
unsafe impl Sync for Ext4FileSystem {}

impl Ext4FileSystem {
    /// Create new ext4 filesystem instance
    pub fn new(device: *const blkdev::GenDisk) -> Self {
        Self {
            device,
            sb_info: None,
            group_descs: spin::Mutex::new(Vec::new()),
            block_size: 4096,
            block_size_bits: 12,
            desc_size: 32,  // Default, will be updated from superblock
            inode_size: 256,
            blocks_per_group: 0,
            inodes_per_group: 0,
            group_count: 0,
            total_blocks: 0,
            total_inodes: 0,
            journal_ino: 0,
            journal: None,
        }
    }

    /// Get group descriptor (read-only)
    pub fn get_group_desc(&self, group: usize) -> Option<superblock::Ext4GroupDesc> {
        let descs = self.group_descs.lock();
        descs.get(group).map(|b| **b)
    }

    /// Get mutable access to group descriptor free blocks count
    pub fn dec_group_free_blocks(&self, group: usize) {
        let mut descs = self.group_descs.lock();
        if group < descs.len() {
            descs[group].bg_free_blocks_count -= 1;
        }
    }

    /// Increment group free blocks count
    pub fn inc_group_free_blocks(&self, group: usize) {
        let mut descs = self.group_descs.lock();
        if group < descs.len() {
            descs[group].bg_free_blocks_count += 1;
        }
    }

    /// Get number of group descriptors
    pub fn group_descs_len(&self) -> usize {
        self.group_descs.lock().len()
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

            // Get descriptor size - use actual size from superblock if 64-bit feature is enabled
            // Default is 32 bytes, but with 64-bit feature it's 64 bytes
            let desc_size = if ext4_sb.s_desc_size < 32 { 32 } else { ext4_sb.s_desc_size as usize };

            // Read block group descriptor table
            // Block group descriptor table starts at block (block_size / 1024) + 1
            let gd_start_block = if block_size == 1024 { 2 } else { 1 };
            let gds_per_block = block_size as usize / desc_size;

            let mut group_descs = Vec::new();

            for i in 0..group_count {
                let gd_block = gd_start_block + (i as usize / gds_per_block) as u32;
                let gd_index = i as usize % gds_per_block;

                let gd_bh = bio::bread(self.device, gd_block as u64)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;

                let gd_data = &(*gd_bh).b_data;
                // Use actual descriptor size for offset calculation
                let gd_offset = gd_index * desc_size;
                let gd_ptr = unsafe {
                    &*(gd_data.as_ptr().add(gd_offset)
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
                s_journal_inum: ext4_sb.s_journal_inum,
            }));

            self.block_size = block_size;
            self.block_size_bits = block_size_bits;
            self.desc_size = desc_size as u16;
            self.inode_size = ext4_sb.s_inode_size;
            self.blocks_per_group = blocks_per_group;
            self.inodes_per_group = inodes_per_group;
            self.group_count = group_count as u32;
            self.total_blocks = total_blocks as u64;
            self.total_inodes = total_inodes;
            self.journal_ino = ext4_sb.s_journal_inum;
            *self.group_descs.lock() = group_descs;

            Ok(())
        }
    }

    /// Read inode
    pub fn read_inode(&self, ino: u32) -> Result<inode::Ext4Inode, i32> {
        // Calculate block group and inode table index
        let group = (ino - 1) / self.inodes_per_group;
        let index = (ino - 1) % self.inodes_per_group;

        let gd = {
            let group_descs = self.group_descs.lock();
            if group as usize >= group_descs.len() {
                return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
            }
            *group_descs[group as usize]
        };

        // Calculate inode block number
        let inode_table_start = gd.bg_inode_table;
        let inodes_per_block = self.block_size / (self.inode_size as u32);
        let inode_block = inode_table_start + (index / inodes_per_block);
        let inode_offset = ((index % inodes_per_block) * (self.inode_size as u32)) as usize;

        // Read block containing inode
        let bh = bio::bread(self.device, inode_block as u64)
            .ok_or(errno::Errno::IOError.as_neg_i32())?;

        let data = unsafe { &(*bh).b_data };

        // Parse inode
        let ext4_inode = unsafe {
            &*(data.as_ptr().add(inode_offset) as *const inode::Ext4InodeOnDisk)
        };

        let result = inode::Ext4Inode::from_disk(ext4_inode, ino);

        bio::brelse(bh);
        Ok(result)
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

            for block in blocks.iter() {
                if *block == 0 {
                    continue;
                }
                let bh = bio::bread(self.device, *block)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;

                let data = &(*bh).b_data;
                let mut offset = 0;

                while offset < self.block_size as usize {
                    let entry = dir::Ext4DirEntry::from_bytes(
                        &data[offset..],
                        self.block_size as usize,
                    );

                    // Guard against corrupted directory entries (rec_len == 0)
                    if entry.rec_len == 0 {
                        break;
                    }

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

            for block in blocks.iter() {
                if *block == 0 {
                    continue;
                }
                let bh = bio::bread(self.device, *block)
                    .ok_or(errno::Errno::IOError.as_neg_i32())?;

                let data = &(*bh).b_data;
                let mut offset = 0;

                while offset < self.block_size as usize {
                    let entry = dir::Ext4DirEntry::from_bytes(
                        &data[offset..],
                        self.block_size as usize,
                    );

                    // Guard against corrupted directory entries (rec_len == 0)
                    if entry.rec_len == 0 {
                        break;
                    }

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
        // Max symlink depth from config
        const MAX_SYMLINK_DEPTH: u32 = crate::config::EXT4_MAX_SYMLINK_DEPTH as u32;

        if symlink_depth > MAX_SYMLINK_DEPTH {
            return Err(errno::Errno::TooManySymbolicLinks.as_neg_i32());
        }

        // Parse path - filter out empty strings and "." (current directory)
        // Note: ".." handling would require parent tracking, not implemented yet
        let path_parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty() && *s != ".").collect();

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

        // Parse path - filter out empty strings and "." (current directory)
        let path_parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty() && *s != ".").collect();

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

    // Initialize journal (gracefully skips if no journal)
    if let Err(e) = fs.init_journal() {
        crate::pr_debug!("ext4: journal init failed: {}", e);
        let _ = e;
    }

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
/// Resolve path to absolute path
/// Supports relative paths and current working directory
/// Always normalizes the path (handles . and ..)
fn resolve_path(path: &str) -> String {
    // Get absolute path
    let abs_path = if path.starts_with('/') {
        // Already absolute path
        String::from(path)
    } else {
        // Get current working directory
        let cwd = if let Some(current) = crate::sched::current() {
            let cwd_bytes = unsafe { (*current).get_cwd() };
            match core::str::from_utf8(&cwd_bytes) {
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
        full_path
    };

    // Always normalize the path (handles . and ..)
    normalize_path(&abs_path)
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

/// Create a VFS inode for the ext4 root directory (inode 2).
/// Called during mount to set up the root dentry's inode.
pub fn create_root_inode() -> alloc::sync::Arc<Inode> {
    let fs_ptr = GLOBAL_EXT4_FS.load(core::sync::atomic::Ordering::Acquire);
    if fs_ptr.is_null() {
        // Fallback: shouldn't happen at mount time
        let mut inode = Inode::new(2, InodeMode::new(InodeMode::S_IFDIR | 0o755));
        inode.ops = Some(&EXT4_INODE_OPS);
        return alloc::sync::Arc::new(inode);
    }
    unsafe {
        let fs = &*fs_ptr;
        match fs.read_inode(2) {
            Ok(ext4_inode) => create_vfs_inode(2, &ext4_inode),
            Err(_) => {
                let mut inode = Inode::new(2, InodeMode::new(InodeMode::S_IFDIR | 0o755));
                inode.ops = Some(&EXT4_INODE_OPS);
                alloc::sync::Arc::new(inode)
            }
        }
    }
}

/// Unmount ext4 filesystem
///
/// This sets the global ext4 filesystem pointer to null and frees the
/// Ext4FileSystem structure. After this call, is_mounted() returns false
/// and all ext4 operations will fail.
pub fn unmount_ext4() {
    use core::sync::atomic::Ordering;

    let fs_ptr = GLOBAL_EXT4_FS.swap(core::ptr::null_mut(), Ordering::AcqRel);
    if !fs_ptr.is_null() {
        unsafe {
            let _ = Box::from_raw(fs_ptr);
        }
    }
}

/// Lookup path in ext4 filesystem and return VFS inode
///
/// # Parameters
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

        // Use the global mounted filesystem directly instead of creating
        // a temporary Ext4FileSystem, which avoids unnecessary buffer
        // cache pressure and potential eviction issues.
        let (_, inode) = fs.lookup_path(&abs_path).ok()?;

        let file_size = inode.get_size() as usize;
        if file_size == 0 {
            return Some(alloc::vec::Vec::new());
        }

        let mut buffer = alloc::vec::Vec::with_capacity(file_size);
        buffer.resize(file_size, 0);

        match file::ext4_file_read(fs, &inode, 0, &mut buffer) {
            Ok(n) => {
                buffer.truncate(n);
                Some(buffer)
            }
            Err(_) => None,
        }
    }
}

/// Create a new file on ext4 filesystem
///
/// # Arguments
/// - `path`: Absolute path for the new file
/// - `mode`: File mode (permissions)
///
/// # Returns
/// - `Ok(inode)`: VFS inode of the created file
/// - `Err(errno)`: Error code
pub fn create_file(path: &str, mode: u32) -> Result<alloc::sync::Arc<Inode>, i32> {
    use core::sync::atomic::Ordering;
    use crate::fs::ext4::inode::{Ext4Inode, file_type};
    use crate::fs::ext4::extent::{Ext4ExtentHeader, EXT4_EXT_MAGIC};

    unsafe {
        let fs_ptr = GLOBAL_EXT4_FS.load(Ordering::Acquire);
        if fs_ptr.is_null() {
            return Err(errno::Errno::IOError.as_neg_i32());
        }
        let fs = &*fs_ptr;

        // Parse path to get parent directory and filename
        let abs_path = resolve_path(path);
        let (parent_path, filename) = split_path(&abs_path);

        // Lookup parent directory, creating intermediate dirs if needed (mkdir -p)
        let parent_inode = create_parent_dirs(fs, &abs_path, &parent_path)?;

        if !parent_inode.is_dir() {
            return Err(errno::Errno::NotADirectory.as_neg_i32());
        }

        // Allocate new inode
        let allocator = allocator::InodeAllocator::new(fs);
        let new_ino = allocator.alloc_inode()?;

        // Initialize new inode
        let mut new_inode = Ext4Inode {
            ino: new_ino,
            mode: (file_type::S_IFREG | (mode as u16 & 0o777)) as u16,
            uid: 0,
            gid: 0,
            size: 0,
            blocks: 0,
            links_count: 1,
            flags: 0x80000,  // EXT4_EXTENTS_FL - use extent tree
            block: [0u32; 15],
            atime: 0,
            mtime: 0,
            ctime: 0,
        };

        // Initialize extent header in i_block
        let header = &mut *(new_inode.block.as_mut_ptr() as *mut Ext4ExtentHeader);
        header.eh_magic = EXT4_EXT_MAGIC;
        header.eh_entries = 0;
        header.eh_max = 4;
        header.eh_depth = 0;
        header.eh_generation = 0;

        // Write new inode to disk
        inode::write_inode(fs, new_ino, &new_inode)?;

        // Add directory entry in parent
        add_dir_entry(fs, &parent_inode, filename, new_ino, 1)?;  // 1 = regular file

        // Create VFS inode
        Ok(create_vfs_inode(new_ino, &new_inode))
    }
}

/// Split path into parent directory and filename
fn split_path(path: &str) -> (&str, &str) {
    let trimmed = path.trim_end_matches('/');
    if let Some(last_slash) = trimmed.rfind('/') {
        let parent = if last_slash == 0 { "/" } else { &trimmed[..last_slash] };
        let name = &trimmed[last_slash + 1..];
        (parent, name)
    } else {
        ("/", path)
    }
}

/// Create parent directories as needed (mkdir -p semantics).
///
/// Tries to lookup `parent_path`; if it fails (ENOENT), creates missing
/// intermediate directories one by one using `ext4_mkdir`.
///
/// Returns the parent directory's Ext4Inode.
fn create_parent_dirs(
    fs: &Ext4FileSystem,
    _abs_path: &str,
    parent_path: &str,
) -> Result<inode::Ext4Inode, i32> {
    // Fast path: parent directory exists and is a directory
    if let Ok((_, inode)) = fs.lookup_path(parent_path) {
        if inode.is_dir() {
            return Ok(inode);
        }
        // A file exists where we need a directory — cannot proceed
        return Err(errno::Errno::NotADirectory.as_neg_i32());
    }

    // Slow path: create intermediate directories one by one.
    // e.g., for "/var/log/kmsg" with parent "/var/log",
    // create "/var" then "/var/log"
    let parts: Vec<&str> = parent_path.split('/').filter(|s| !s.is_empty()).collect();
    let mut current_ino = 2u32; // root inode

    for (i, part) in parts.iter().enumerate() {
        // Build the full path up to this component, e.g., "/var", "/var/log"
        let mut path = alloc::string::String::from("/");
        for (j, p) in parts[..=i].iter().enumerate() {
            if j > 0 {
                path.push('/');
            }
            path.push_str(p);
        }

        match fs.lookup_path(&path) {
            Ok((ino, inode)) => {
                if !inode.is_dir() {
                    return Err(errno::Errno::NotADirectory.as_neg_i32());
                }
                current_ino = ino;
            }
            Err(_) => {
                // Directory doesn't exist, create it
                current_ino = crate::fs::ext4::namei::ext4_mkdir(
                    fs,
                    current_ino,
                    part.as_bytes(),
                    0o755,
                )?;
            }
        }
    }

    // Read the final parent inode
    fs.read_inode(current_ino)
}

/// Add a directory entry
fn add_dir_entry(
    fs: &Ext4FileSystem,
    parent: &inode::Ext4Inode,
    name: &str,
    ino: u32,
    file_type: u8,
) -> Result<(), i32> {
    use crate::fs::bio;

    // Get parent's data blocks
    let blocks = parent.get_data_blocks(fs)?;

    if blocks.is_empty() {
        // Parent has no data blocks, need to allocate one
        return Err(errno::Errno::IOError.as_neg_i32());
    }

    let block_size = fs.block_size as usize;
    let entry_size = ((8 + name.len() as usize + 3) / 4) * 4;

    // Iterate all blocks looking for space
    for block_num in &blocks {
        if *block_num == 0 {
            continue;  // Skip sparse blocks
        }

        unsafe {
            let bh = bio::bread(fs.device, *block_num)
                .ok_or(errno::Errno::IOError.as_neg_i32())?;
            let data = &mut (*bh).b_data;

            // Try to find space in this block
            let mut offset = 0;
            let mut prev_rec_len = 0usize;

            while offset + 8 < block_size {
                let rec_len = u16::from_le_bytes([data[offset + 4], data[offset + 5]]) as usize;

                if rec_len == 0 {
                    break;
                }

                // Check if this is an unused entry (inode == 0) with enough space
                let existing_ino = u32::from_le_bytes([
                    data[offset], data[offset + 1], data[offset + 2], data[offset + 3]
                ]);

                if existing_ino == 0 && rec_len >= entry_size {
                    // Reuse this entry
                    let entry_data = &mut data[offset..offset + entry_size];
                    create_dir_entry(entry_data, ino, name, file_type, rec_len as u16);
                    (*bh).set_state_bit(bio::BufferState::BH_Dirty);
                    bio::sync_dirty_buffer(bh)?;
                    bio::brelse(bh);
                    return Ok(());
                }

                prev_rec_len = rec_len;
                offset += rec_len;
            }

            // Try to add at end of this block
            if offset + entry_size <= block_size {
                // Update previous entry's rec_len to point to new entry
                if prev_rec_len > 0 && offset >= prev_rec_len {
                    let prev_offset = offset - prev_rec_len;
                    let prev_name_len = data[prev_offset + 6];
                    let prev_actual_size = ((8 + prev_name_len as usize + 3) / 4) * 4;
                    data[prev_offset + 4] = (prev_actual_size & 0xFF) as u8;
                    data[prev_offset + 5] = ((prev_actual_size >> 8) & 0xFF) as u8;
                }

                // Create new entry
                let remaining = block_size - offset;
                let entry_data = &mut data[offset..offset + entry_size];
                create_dir_entry(entry_data, ino, name, file_type, remaining as u16);

                (*bh).set_state_bit(bio::BufferState::BH_Dirty);
                bio::sync_dirty_buffer(bh)?;
                bio::brelse(bh);
                return Ok(());
            }

            bio::brelse(bh);
        }
    }

    // All blocks are full, need to allocate a new block
    append_dir_block(fs, parent, name, ino, file_type, entry_size, block_size)
}

/// Append a new block to directory and add entry
fn append_dir_block(
    fs: &Ext4FileSystem,
    parent: &inode::Ext4Inode,
    name: &str,
    ino: u32,
    file_type: u8,
    entry_size: usize,
    block_size: usize,
) -> Result<(), i32> {
    use crate::fs::bio;

    // Read parent inode from disk first
    let parent_ino = parent.ino;
    let mut parent_inode = fs.read_inode(parent_ino)?;

    // Get current block count
    let current_blocks = (parent_inode.get_size() + block_size as u64 - 1) / block_size as u64;
    let new_block_index = current_blocks;

    // First allocate the block in the inode's extent tree/indirect blocks
    // This will give us the physical block number
    file::allocate_blocks_for_file(fs, &mut parent_inode, new_block_index + 1)?;

    // Now get the actual block number that was allocated
    let new_block = parent_inode.get_data_block(fs, new_block_index)?;

    // Zero the new block and create the directory entry
    unsafe {
        let bh = bio::bread(fs.device, new_block)
            .ok_or(errno::Errno::IOError.as_neg_i32())?;

        // Zero the block
        for byte in (*bh).b_data.iter_mut() {
            *byte = 0;
        }

        // Create a single entry spanning the whole block
        // Since we don't have checksum, use full block size
        let data = &mut (*bh).b_data;
        create_dir_entry(data, ino, name, file_type, block_size as u16);

        (*bh).set_state_bit(bio::BufferState::BH_Dirty);
        bio::sync_dirty_buffer(bh)?;
        bio::brelse(bh);
    }

    // Update parent directory size
    let new_size = (new_block_index + 1) as u64 * block_size as u64;
    parent_inode.set_size(new_size);

    // Write parent inode back to disk
    inode::write_inode(fs, parent_ino, &parent_inode)?;

    Ok(())
}

/// Create a directory entry in buffer
fn create_dir_entry(data: &mut [u8], ino: u32, name: &str, file_type: u8, rec_len: u16) {
    // inode number (4 bytes)
    data[0..4].copy_from_slice(&ino.to_le_bytes());
    // record length (2 bytes)
    data[4..6].copy_from_slice(&rec_len.to_le_bytes());
    // name length (1 byte)
    data[6] = name.len() as u8;
    // file type (1 byte)
    data[7] = file_type;
    // name (variable)
    data[8..8 + name.len()].copy_from_slice(name.as_bytes());
}

// ============================================================================
// Ext4 Inode Operations
// ============================================================================

use crate::fs::inode::{Inode, InodeMode, INodeOps, Ino};
use crate::fs::Stat;

/// Ext4 inode lookup operation
unsafe fn ext4_lookup(dir: &Inode, name: &[u8]) -> Result<Ino, i32> {
    let fs_ptr = dir.private_data.ok_or(errno::Errno::IOError.as_neg_i32())?;
    let fs = &*(fs_ptr as *const Ext4FileSystem);

    // Get ext4 inode from parent's private_data
    let parent_ext4_inode = dir.sb.ok_or(errno::Errno::NotADirectory.as_neg_i32())?;
    let parent_inode = &*(parent_ext4_inode as *const inode::Ext4Inode);

    // Convert name to str
    let name_str = core::str::from_utf8(name).map_err(|_| errno::Errno::InvalidArgument.as_neg_i32())?;

    // Lookup in directory
    let entry = fs.lookup(parent_inode, name_str)?;
    Ok(entry.inode as Ino)
}

/// Ext4 getattr operation
unsafe fn ext4_getattr(inode: &Inode, stat: &mut Stat) -> i32 {
    let fs_ptr = match inode.private_data {
        Some(ptr) => ptr,
        None => return errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    };
    let _fs = &*(fs_ptr as *const Ext4FileSystem);

    // Get ext4 inode from sb field (we store it there)
    let ext4_inode_ptr = match inode.sb {
        Some(ptr) => ptr,
        None => return errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    };
    let ext4_inode = &*(ext4_inode_ptr as *const inode::Ext4Inode);

    stat.st_ino = inode.ino;
    stat.st_mode = ext4_inode.mode as u32;  // u16 -> u32
    stat.st_size = ext4_inode.get_size() as i64;
    stat.st_nlink = ext4_inode.links_count as u32;
    stat.st_uid = ext4_inode.uid as u32;
    stat.st_gid = ext4_inode.gid as u32;
    stat.st_rdev = 0;
    stat.st_blksize = 4096;
    stat.st_blocks = ext4_inode.blocks as i64;
    stat.st_atime = ext4_inode.atime as i64;
    stat.st_atime_nsec = 0;
    stat.st_mtime = ext4_inode.mtime as i64;
    stat.st_mtime_nsec = 0;
    stat.st_ctime = ext4_inode.ctime as i64;
    stat.st_ctime_nsec = 0;

    0
}

/// Ext4 readlink operation
unsafe fn ext4_readlink(inode: &Inode, buf: &mut [u8]) -> isize {
    let fs_ptr = match inode.private_data {
        Some(ptr) => ptr,
        None => return errno::Errno::InvalidArgument.as_neg_i32() as isize,
    };
    let fs = &*(fs_ptr as *const Ext4FileSystem);

    // Get ext4 inode from sb field
    let ext4_inode_ptr = match inode.sb {
        Some(ptr) => ptr,
        None => return errno::Errno::InvalidArgument.as_neg_i32() as isize,
    };
    let ext4_inode = &*(ext4_inode_ptr as *const inode::Ext4Inode);

    if !ext4_inode.is_symlink() {
        return errno::Errno::InvalidArgument.as_neg_i32() as isize;
    }

    // Read symlink target
    let size = ext4_inode.get_size() as usize;
    if size == 0 || size > buf.len() {
        return errno::Errno::IOError.as_neg_i32() as isize;
    }

    // Short symlink: data stored inline
    if size <= 60 && !ext4_inode.has_extent() {
        let block_data = core::slice::from_raw_parts(
            ext4_inode.block.as_ptr() as *const u8,
            60
        );
        buf[..size].copy_from_slice(&block_data[..size]);
        size as isize
    } else {
        // Long symlink: read from data blocks
        match ext4_inode.read_data(fs, 0, &mut buf[..size]) {
            Ok(n) if n == size => n as isize,
            _ => errno::Errno::IOError.as_neg_i32() as isize,
        }
    }
}

/// Ext4 setattr implementation
///
/// Handles chmod (ATTR_MODE), chown (ATTR_UID_GID), and ftruncate (ATTR_SIZE)
unsafe fn ext4_setattr(inode: &Inode, attr: u32, arg1: u64, arg2: u64) -> i32 {
    use crate::fs::inode::setattr_attr;
    use crate::drivers::intc::clint::read_time;

    let fs = match get_ext4_fs_from_inode(inode) {
        Ok(fs) => fs,
        Err(e) => return e,
    };
    let ext4_ino = inode.ino as u32;

    let mut ext4_inode = match fs.read_inode(ext4_ino) {
        Ok(i) => i,
        Err(e) => return e,
    };

    match attr {
        setattr_attr::ATTR_MODE => {
            // arg1 = new mode (permission bits only, file type preserved)
            let new_mode = (arg1 as u32) & 0o777;
            ext4_inode.mode = (ext4_inode.mode & 0xF000) | (new_mode as u16);
        }
        setattr_attr::ATTR_UID_GID => {
            // arg1 = uid, arg2 = gid
            ext4_inode.uid = arg1 as u16;
            ext4_inode.gid = arg2 as u16;
        }
        setattr_attr::ATTR_SIZE => {
            // arg1 = new size (ftruncate)
            let new_size = arg1;
            if new_size < ext4_inode.get_size() {
                // Truncate: free blocks beyond new_size
                let allocator = crate::fs::ext4::allocator::BlockAllocator::new(fs);
                let block_size = fs.block_size as u64;
                let new_blocks = (new_size + block_size - 1) / block_size;
                let old_blocks = (ext4_inode.get_size() + block_size - 1) / block_size;

                if ext4_inode.has_extent() {
                    use crate::fs::ext4::extent::{Ext4ExtentHeader, Ext4Extent, EXT4_EXT_MAGIC};
                    let header = &*(ext4_inode.block.as_ptr() as *const Ext4ExtentHeader);
                    if header.eh_magic == EXT4_EXT_MAGIC {
                        let entries = core::slice::from_raw_parts(
                            (ext4_inode.block.as_ptr() as *const u8)
                                .add(core::mem::size_of::<Ext4ExtentHeader>())
                                as *const Ext4Extent,
                            header.eh_entries as usize
                        );
                        for ext in entries {
                            let ext_start = ext.start_block();
                            let ext_len = ext.length() as u64;
                            // Free blocks that are entirely beyond new_blocks
                            if ext_start >= new_blocks {
                                for j in 0..ext_len {
                                    let _ = allocator.free_block(ext_start + j);
                                }
                            } else if ext_start + ext_len > new_blocks {
                                for j in new_blocks - ext_start..ext_len {
                                    let _ = allocator.free_block(ext_start + j);
                                }
                            }
                        }
                    }
                } else {
                    // Free indirect blocks beyond new size
                    for i in new_blocks as usize..old_blocks as usize {
                        if i < 12 {
                            if ext4_inode.block[i] != 0 {
                                let _ = allocator.free_block(ext4_inode.block[i] as u64);
                                ext4_inode.block[i] = 0;
                            }
                        } else {
                            // Indirect blocks - use ext4_get_block to check, then free
                            match indirect::ext4_get_block(fs, &ext4_inode.block, i as u64) {
                                Ok(block_num) if block_num != 0 => {
                                    let _ = allocator.free_block(block_num);
                                }
                                _ => {}
                            }
                        }
                    }
                    // Clean up indirect block pointers if truncating below 12 blocks
                    if new_blocks < 12 && ext4_inode.block[12] != 0 {
                        let _ = allocator.free_block(ext4_inode.block[12] as u64);
                        ext4_inode.block[12] = 0;
                    }
                    if new_blocks < 12 + (block_size / 4) as u64 && ext4_inode.block[13] != 0 {
                        let _ = allocator.free_block(ext4_inode.block[13] as u64);
                        ext4_inode.block[13] = 0;
                    }
                }
                ext4_inode.blocks = (new_blocks * (block_size / 512)) as u64;
            }
            ext4_inode.set_size(new_size);
        }
        _ => return errno::Errno::InvalidArgument.as_neg_i32(),
    }

    // Update timestamps
    let cycles = read_time();
    let sec = (cycles / 10_000_000) as u32;
    ext4_inode.mtime = sec;
    ext4_inode.ctime = sec;

    // Write back
    match inode::write_inode(fs, ext4_ino, &ext4_inode) {
        Ok(()) => {
            // Refresh cached Ext4Inode so subsequent reads see the new state
            refresh_inode_cache(inode, fs);
            // Invalidate page cache after size change (truncate/extend)
            crate::fs::page_cache::get_page_cache().invalidate_inode(inode.ino as u32);
            0
        }
        Err(e) => e,
    }
}

/// Ext4 inode operations table
/// Ext4 now supports write operations through namei module
pub static EXT4_INODE_OPS: INodeOps = INodeOps {
    lookup: Some(ext4_lookup),
    create: Some(ext4_create_wrapper),
    link: Some(ext4_link_wrapper),
    unlink: Some(ext4_unlink_wrapper),
    symlink: None,      // TODO: implement
    mkdir: Some(ext4_mkdir_wrapper),
    rmdir: Some(ext4_rmdir_wrapper),
    mknod: None,        // TODO: implement
    rename: Some(ext4_rename_wrapper),
    readlink: Some(ext4_readlink),
    get_file_ops: Some(ext4_get_file_ops),
    readdir: Some(ext4_readdir),
    open: None,
    permission: None,   // Default: allow all
    getattr: Some(ext4_getattr),
    setattr: Some(ext4_setattr),
    iget: Some(ext4_iget),
};

/// Ext4 iget: instantiate VFS Inode from (parent, name, ino).
///
/// Reads the child inode from disk using the parent's filesystem pointer.
unsafe fn ext4_iget(parent: &Inode, _name: &[u8], ino: Ino) -> Result<alloc::sync::Arc<Inode>, i32> {
    let fs_ptr = parent.private_data.ok_or(errno::Errno::IOError.as_neg_i32())?;
    let fs = &*(fs_ptr as *const Ext4FileSystem);

    // Read child inode from disk
    let ext4_inode = fs.read_inode(ino as u32)
        .map_err(|_| errno::Errno::NoSuchFileOrDirectory.as_neg_i32())?;

    let vfs_inode = create_vfs_inode(ino as u32, &ext4_inode);
    crate::fs::inode::icache_add(vfs_inode.clone());
    Ok(vfs_inode)
}

/// Wrapper for ext4_mkdir to match VFS signature
unsafe fn ext4_mkdir_wrapper(dir: &Inode, name: &[u8], mode: InodeMode) -> Result<alloc::sync::Arc<Inode>, i32> {
    let fs = get_ext4_fs_from_inode(dir)?;

    // Call namei's ext4_mkdir
    let new_ino = namei::ext4_mkdir(fs, dir.ino as u32, name, mode.bits() as u16)?;

    // Update parent directory's cached Ext4Inode
    refresh_parent_dir_cache(dir, fs);

    // Read the new inode and convert to in-memory format
    let disk_inode = inode::read_inode(fs, new_ino)?;
    let ext4_inode = inode::Ext4Inode::from_disk(&disk_inode, new_ino);
    let vfs_inode = create_vfs_inode(new_ino, &ext4_inode);
    crate::fs::inode::icache_add(vfs_inode.clone());
    Ok(vfs_inode)
}

/// Update the parent directory's cached Ext4Inode after a directory modification.
/// This ensures subsequent lookups see the latest block pointers and size.
unsafe fn refresh_parent_dir_cache(dir: &Inode, fs: &Ext4FileSystem) {
    refresh_inode_cache(dir, fs);
}

/// Refresh the Ext4Inode cached in inode.sb after a disk write.
/// This ensures cached data (size, blocks, timestamps) stays in sync.
unsafe fn refresh_inode_cache(inode: &Inode, fs: &Ext4FileSystem) {
    if let Some(sb_ptr) = inode.sb {
        if let Ok(disk_inode) = inode::read_inode(fs, inode.ino as u32) {
            let updated = inode::Ext4Inode::from_disk(&disk_inode, inode.ino as u32);
            let cached = &mut *(sb_ptr as *mut inode::Ext4Inode);
            *cached = updated;
        }
    }
}

/// Wrapper for ext4_rmdir to match VFS signature
unsafe fn ext4_rmdir_wrapper(dir: &Inode, name: &[u8]) -> i32 {
    let fs = match get_ext4_fs_from_inode(dir) {
        Ok(f) => f,
        Err(e) => return e,
    };

    match namei::ext4_rmdir(fs, dir.ino as u32, name) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// Wrapper for ext4_create to match VFS signature
unsafe fn ext4_create_wrapper(dir: &Inode, name: &[u8], mode: InodeMode) -> Result<alloc::sync::Arc<Inode>, i32> {
    let fs = get_ext4_fs_from_inode(dir)?;
    let new_ino = namei::ext4_create(fs, dir.ino as u32, name, mode.bits() as u16)?;

    // Update parent directory's cached Ext4Inode
    refresh_parent_dir_cache(dir, fs);

    let disk_inode = inode::read_inode(fs, new_ino)?;
    let ext4_inode = inode::Ext4Inode::from_disk(&disk_inode, new_ino);
    let vfs_inode = create_vfs_inode(new_ino, &ext4_inode);
    crate::fs::inode::icache_add(vfs_inode.clone());
    Ok(vfs_inode)
}

/// Wrapper for ext4_link to match VFS signature
unsafe fn ext4_link_wrapper(dir: &Inode, name: &[u8], target: &Inode) -> i32 {
    let fs = match get_ext4_fs_from_inode(dir) {
        Ok(f) => f,
        Err(e) => return e,
    };

    match namei::ext4_link(fs, dir.ino as u32, target.ino as u32, name) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// Wrapper for ext4_unlink to match VFS signature
unsafe fn ext4_unlink_wrapper(dir: &Inode, name: &[u8]) -> i32 {
    let fs = match get_ext4_fs_from_inode(dir) {
        Ok(f) => f,
        Err(e) => return e,
    };

    let result = match namei::ext4_unlink(fs, dir.ino as u32, name) {
        Ok(()) => 0,
        Err(e) => e,
    };

    // Update parent directory's cached Ext4Inode
    if result == 0 {
        refresh_parent_dir_cache(dir, fs);
    }

    result
}

/// Wrapper for ext4_rename to match VFS signature
unsafe fn ext4_rename_wrapper(old_dir: &Inode, old_name: &[u8], new_dir: &Inode, new_name: &[u8]) -> i32 {
    let fs = match get_ext4_fs_from_inode(old_dir) {
        Ok(f) => f,
        Err(e) => return e,
    };

    match namei::ext4_rename(fs, old_dir.ino as u32, old_name, new_dir.ino as u32, new_name) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

/// Get Ext4FileSystem pointer from inode's private_data
fn get_ext4_fs_from_inode(inode: &Inode) -> Result<&'static Ext4FileSystem, i32> {
    let fs_ptr = inode.private_data.ok_or(errno::Errno::IOError.as_neg_i32())?;
    unsafe {
        Ok(&*(fs_ptr as *const Ext4FileSystem))
    }
}

/// Get file operations for ext4 regular files and directories
unsafe fn ext4_get_file_ops(inode: &Inode) -> Option<&'static crate::fs::file::FileOps> {
    if inode.mode.is_regular_file() {
        Some(&file::EXT4_FILE_OPS)
    } else if inode.mode.is_directory() {
        Some(&crate::fs::file::DIR_FILE_OPS)
    } else {
        None
    }
}

/// Ext4 readdir: list directory entries via inode.ops
unsafe fn ext4_readdir(inode: &Inode) -> Option<alloc::vec::Vec<crate::fs::inode::VfsDirEntry>> {
    use crate::fs::inode::file_type;

    let fs_ptr = get_ext4_fs_from_inode(inode).ok()?;
    let ext4_inode_ptr = inode.sb?;
    let ext4_inode = &*(ext4_inode_ptr as *const inode::Ext4Inode);

    let ext4_entries = fs_ptr.list_dir(ext4_inode).ok()?;
    let mut entries = alloc::vec::Vec::new();
    for entry in ext4_entries.iter() {
        let name_bytes = &entry.name[..entry.name_len as usize];
        let dt = match entry.file_type {
            1 => file_type::DT_REG,
            2 => file_type::DT_DIR,
            3 => file_type::DT_CHR,
            4 => file_type::DT_BLK,
            5 => file_type::DT_FIFO,
            6 => file_type::DT_SOCK,
            7 => file_type::DT_LNK,
            _ => file_type::DT_UNKNOWN,
        };
        entries.push(crate::fs::inode::VfsDirEntry {
            ino: entry.inode as u64,
            name: name_bytes.to_vec(),
            file_type: dt,
        });
    }
    Some(entries)
}

/// Create VFS inode from ext4 inode
///
/// This helper function creates a VFS inode structure from an ext4 inode,
/// properly setting up the inode_operations and private data.
pub fn create_vfs_inode(ino: u32, ext4_inode: &inode::Ext4Inode) -> alloc::sync::Arc<Inode> {
    let mode = if ext4_inode.is_dir() {
        InodeMode::new(InodeMode::S_IFDIR | (ext4_inode.mode as u32 & 0o777))
    } else if ext4_inode.is_symlink() {
        InodeMode::new(InodeMode::S_IFLNK | 0o777)
    } else if ext4_inode.is_reg() {
        InodeMode::new(InodeMode::S_IFREG | (ext4_inode.mode as u32 & 0o777))
    } else {
        InodeMode::new(ext4_inode.mode as u32)
    };

    // Store ext4 filesystem pointer in private_data
    let fs_ptr = GLOBAL_EXT4_FS.load(core::sync::atomic::Ordering::Acquire);

    let mut inode = Inode::new(ino as u64, mode);
    // Set fs_id to the filesystem pointer address for cache uniqueness
    inode.fs_id = fs_ptr as u64;
    inode.uid.store(ext4_inode.uid as u32, core::sync::atomic::Ordering::Relaxed);
    inode.gid.store(ext4_inode.gid as u32, core::sync::atomic::Ordering::Relaxed);
    inode.size.store(ext4_inode.size, core::sync::atomic::Ordering::Relaxed);
    inode.ops = Some(&EXT4_INODE_OPS);
    inode.private_data = Some(fs_ptr as *mut u8);

    // Cache a copy of the Ext4Inode in sb field (boxed, leaked pointer)
    // This avoids re-reading from disk on every read/write/stat/lseek
    let ext4_copy = alloc::boxed::Box::new(ext4_inode.clone());
    inode.sb = Some(Box::into_raw(ext4_copy) as *const u8);

    alloc::sync::Arc::new(inode)
}
