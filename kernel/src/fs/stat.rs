//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! File Status Information (stat)

/// File status information
///
/// ...
///
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Stat {
    /// Device ID (st_dev)
    /// If it's a file, contains the device ID where the file is located
    pub st_dev: u64,

    /// Inode number (st_ino)
    pub st_ino: u64,

    /// File type and permissions (st_mode)
    ///
    /// Bit masks:
    /// - S_IFMT  (0o170000) - File type mask
    /// - S_IFREG (0o100000) - Regular file
    /// - S_IFDIR (0o040000) - Directory
    /// - S_IFCHR (0o020000) - Character device
    /// - S_IFBLK (0o060000) - Block device
    /// - S_IFIFO (0o010000) - FIFO
    /// - S_IFLNK (0o120000) - Symbolic link
    /// - S_IFSOCK(0o140000) - socket
    /// - Permission bits: 0o777 (rwxrwxrwx)
    pub st_mode: u32,

    /// Hard link count (st_nlink)
    pub st_nlink: u32,

    /// User ID (st_uid)
    pub st_uid: u32,

    /// Group ID (st_gid)
    pub st_gid: u32,

    /// Device ID (if special file) (st_rdev)
    pub st_rdev: u64,

    /// File size (bytes) (st_size)
    pub st_size: i64,

    /// Block size (st_blksize)
    /// Preferred block size for filesystem I/O
    pub st_blksize: u64,

    /// Number of 512-byte blocks allocated (st_blocks)
    pub st_blocks: u64,

    /// Last access time (st_atime)
    pub st_atime: u64,

    /// Nanoseconds part of last access time (st_atime_nsec)
    pub st_atime_nsec: u64,

    /// Last modification time (st_mtime)
    pub st_mtime: u64,

    /// Nanoseconds part of last modification time (st_mtime_nsec)
    pub st_mtime_nsec: u64,

    /// Last status change time (st_ctime)
    pub st_ctime: u64,

    /// Nanoseconds part of last status change time (st_ctime_nsec)
    pub st_ctime_nsec: u64,
}

impl Stat {
    /// Create default Stat structure
    pub fn new() -> Self {
        Self {
            st_dev: 0,
            st_ino: 0,
            st_mode: 0,
            st_nlink: 0,
            st_uid: 0,
            st_gid: 0,
            st_rdev: 0,
            st_size: 0,
            st_blksize: 4096,  // Default 4KB
            st_blocks: 0,
            st_atime: 0,
            st_atime_nsec: 0,
            st_mtime: 0,
            st_mtime_nsec: 0,
            st_ctime: 0,
            st_ctime_nsec: 0,
        }
    }

    /// Set as regular file
    pub fn set_regular_file(&mut self) {
        self.st_mode = (self.st_mode & !0o170000) | 0o100000;
    }

    /// Set as directory
    pub fn set_directory(&mut self) {
        self.st_mode = (self.st_mode & !0o170000) | 0o040000;
    }

    /// Set as character device
    pub fn set_char_device(&mut self) {
        self.st_mode = (self.st_mode & !0o170000) | 0o020000;
    }

    /// Set as block device
    pub fn set_block_device(&mut self) {
        self.st_mode = (self.st_mode & !0o170000) | 0o060000;
    }

    /// Set as FIFO
    pub fn set_fifo(&mut self) {
        self.st_mode = (self.st_mode & !0o170000) | 0o010000;
    }

    /// Set as symbolic link
    pub fn set_symlink(&mut self) {
        self.st_mode = (self.st_mode & !0o170000) | 0o120000;
    }

    /// Set as socket
    pub fn set_socket(&mut self) {
        self.st_mode = (self.st_mode & !0o170000) | 0o140000;
    }

    /// Check if regular file
    pub fn is_regular_file(&self) -> bool {
        (self.st_mode & 0o170000) == 0o100000
    }

    /// Check if directory
    pub fn is_directory(&self) -> bool {
        (self.st_mode & 0o170000) == 0o040000
    }

    /// Check if character device
    pub fn is_char_device(&self) -> bool {
        (self.st_mode & 0o170000) == 0o020000
    }

    /// Check if block device
    pub fn is_block_device(&self) -> bool {
        (self.st_mode & 0o170000) == 0o060000
    }

    /// Check if FIFO
    pub fn is_fifo(&self) -> bool {
        (self.st_mode & 0o170000) == 0o010000
    }

    /// Check if symbolic link
    pub fn is_symlink(&self) -> bool {
        (self.st_mode & 0o170000) == 0o120000
    }

    /// Check if socket
    pub fn is_socket(&self) -> bool {
        (self.st_mode & 0o170000) == 0o140000
    }

    /// Set permission bits
    pub fn set_mode(&mut self, mode: u32) {
        // Clear low 9 bits of permissions
        self.st_mode &= 0o170000;
        // Set new permissions
        self.st_mode |= mode & 0o777;
    }

    /// Get permission bits
    pub fn get_mode(&self) -> u32 {
        self.st_mode & 0o777
    }
}

impl Default for Stat {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stat_creation() {
        let stat = Stat::new();
        assert_eq!(stat.st_dev, 0);
        assert_eq!(stat.st_ino, 0);
        assert_eq!(stat.st_size, 0);
    }

    #[test]
    fn test_file_type() {
        let mut stat = Stat::new();

        stat.set_regular_file();
        assert!(stat.is_regular_file());
        assert!(!stat.is_directory());

        stat.set_directory();
        assert!(stat.is_directory());
        assert!(!stat.is_regular_file());
    }

    #[test]
    fn test_permissions() {
        let mut stat = Stat::new();
        stat.set_mode(0o644);

        assert_eq!(stat.get_mode(), 0o644);
    }
}
