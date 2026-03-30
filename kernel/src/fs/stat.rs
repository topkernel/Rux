//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! File Status Information (stat)

/// File status information
///
/// ...
///
/// Layout matches musl libc for RISC-V 64-bit:
/// - st_dev: u64 @ 0
/// - st_ino: u64 @ 8
/// - st_mode: u32 @ 16
/// - st_nlink: u32 @ 20
/// - st_uid: u32 @ 24
/// - st_gid: u32 @ 28
/// - st_rdev: u64 @ 32
/// - __pad: u64 @ 40
/// - st_size: i64 @ 48
/// - st_blksize: i64 @ 56 (musl uses long, not int)
/// - st_blocks: i64 @ 64
/// - st_atime: i64 @ 72
/// - st_atime_nsec: u64 @ 80
/// - st_mtime: i64 @ 88
/// - st_mtime_nsec: u64 @ 96
/// - st_ctime: i64 @ 104
/// - st_ctime_nsec: u64 @ 112
/// - __unused: [u64; 2] @ 120 (padding to 128 bytes)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Stat {
    /// Device ID (st_dev)
    pub st_dev: u64,

    /// Inode number (st_ino)
    pub st_ino: u64,

    /// File type and permissions (st_mode)
    pub st_mode: u32,

    /// Hard link count (st_nlink)
    pub st_nlink: u32,

    /// User ID (st_uid)
    pub st_uid: u32,

    /// Group ID (st_gid)
    pub st_gid: u32,

    /// Device ID (if special file) (st_rdev)
    pub st_rdev: u64,

    /// Padding (musl expects gap here)
    __pad1: u64,

    /// File size (bytes) (st_size)
    pub st_size: i64,

    /// Block size (st_blksize) - musl uses long (64-bit on riscv64)
    pub st_blksize: i64,

    /// Number of 512-byte blocks allocated (st_blocks)
    pub st_blocks: i64,

    /// Last access time (st_atime)
    pub st_atime: i64,

    /// Nanoseconds part of last access time (st_atime_nsec)
    pub st_atime_nsec: u64,

    /// Last modification time (st_mtime)
    pub st_mtime: i64,

    /// Nanoseconds part of last modification time (st_mtime_nsec)
    pub st_mtime_nsec: u64,

    /// Last status change time (st_ctime)
    pub st_ctime: i64,

    /// Nanoseconds part of last status change time (st_ctime_nsec)
    pub st_ctime_nsec: u64,

    /// Unused padding to match 128 byte struct size
    __unused: [u32; 2],
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
            __pad1: 0,
            st_size: 0,
            st_blksize: 4096,  // Default 4KB
            st_blocks: 0,
            st_atime: 0,
            st_atime_nsec: 0,
            st_mtime: 0,
            st_mtime_nsec: 0,
            st_ctime: 0,
            st_ctime_nsec: 0,
            __unused: [0, 0],
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

/// Extended file status information (statx)
///
/// Layout matches Linux struct statx for RISC-V 64-bit (256 bytes).
/// Reference: linux/include/uapi/linux/stat.h
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct Statx {
    /// Mask of what was filled in (STATX_*)
    pub stx_mask: u32,
    /// Preferred block size for I/O
    pub stx_blksize: u32,
    /// Mount-specific flags
    pub stx_attributes: u64,
    /// Number of hard links
    pub stx_nlink: u32,
    /// User ID
    pub stx_uid: u32,
    /// Group ID
    pub stx_gid: u32,
    /// File mode (type + permissions)
    pub stx_mode: u16,
    __spare0: u16,
    /// Inode number
    pub stx_ino: u64,
    /// File size (bytes)
    pub stx_size: u64,
    /// Number of 512-byte blocks allocated
    pub stx_blocks: u64,
    /// Mask for stx_attributes
    pub stx_attributes_mask: u64,
    /// Last access time
    pub stx_atime: StatxTimestamp,
    /// Creation time (btime)
    pub stx_btime: StatxTimestamp,
    /// Last status change time
    pub stx_ctime: StatxTimestamp,
    /// Last modification time
    pub stx_mtime: StatxTimestamp,
    /// Device major for special files
    pub stx_rdev_major: u32,
    /// Device minor for special files
    pub stx_rdev_minor: u32,
    /// Device major of filesystem
    pub stx_dev_major: u32,
    /// Device minor of filesystem
    pub stx_dev_minor: u32,
    /// Spare space to reach 256 bytes
    __spare2: [u64; 14],
}

/// Timestamp for statx
#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct StatxTimestamp {
    /// Seconds since epoch
    pub tv_sec: i64,
    /// Nanoseconds
    pub tv_nsec: u32,
    /// Reserved
    __reserved: i32,
}

impl Statx {
    /// Create zeroed Statx structure
    pub fn new() -> Self {
        Self {
            stx_mask: 0,
            stx_blksize: 0,
            stx_attributes: 0,
            stx_nlink: 0,
            stx_uid: 0,
            stx_gid: 0,
            stx_mode: 0,
            __spare0: 0,
            stx_ino: 0,
            stx_size: 0,
            stx_blocks: 0,
            stx_attributes_mask: 0,
            stx_atime: StatxTimestamp { tv_sec: 0, tv_nsec: 0, __reserved: 0 },
            stx_btime: StatxTimestamp { tv_sec: 0, tv_nsec: 0, __reserved: 0 },
            stx_ctime: StatxTimestamp { tv_sec: 0, tv_nsec: 0, __reserved: 0 },
            stx_mtime: StatxTimestamp { tv_sec: 0, tv_nsec: 0, __reserved: 0 },
            stx_rdev_major: 0,
            stx_rdev_minor: 0,
            stx_dev_major: 0,
            stx_dev_minor: 0,
            __spare2: [0; 14],
        }
    }
}

impl Default for Statx {
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
