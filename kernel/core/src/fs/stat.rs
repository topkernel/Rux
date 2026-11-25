//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!

//! File Status Information (stat)

// The type definitions this file declared live in fs/types.rs; every impl
// and function that uses them stays in this file. Re-exported so the
// existing `fs::stat::…` paths are unchanged.
pub use super::types::{Stat, Statx, StatxTimestamp};
#[cfg(feature = "x86_64")]
pub use super::types::X86Stat;

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

#[cfg(feature = "x86_64")]
impl From<&Stat> for X86Stat {
    fn from(s: &Stat) -> Self {
        X86Stat {
            st_dev: s.st_dev,
            st_ino: s.st_ino,
            st_nlink: s.st_nlink as u64,
            st_mode: s.st_mode,
            st_uid: s.st_uid,
            st_gid: s.st_gid,
            __pad0: 0,
            st_rdev: s.st_rdev,
            st_size: s.st_size,
            st_blksize: s.st_blksize,
            st_blocks: s.st_blocks,
            st_atime: s.st_atime,
            st_atime_nsec: s.st_atime_nsec as i64,
            st_mtime: s.st_mtime,
            st_mtime_nsec: s.st_mtime_nsec as i64,
            st_ctime: s.st_ctime,
            st_ctime_nsec: s.st_ctime_nsec as i64,
            __glibc_reserved: [0; 3],
        }
    }
}

/// Size of the stat struct as the current build target's userspace ABI
/// expects it (access_ok sizing and copy length).
pub fn user_stat_size() -> usize {
    #[cfg(feature = "x86_64")]
    return core::mem::size_of::<X86Stat>();
    #[cfg(not(feature = "x86_64"))]
    core::mem::size_of::<Stat>()
}

/// Copy `stat` to a validated user buffer in the target ABI layout.
/// Returns the number of bytes NOT copied (0 = success), matching
/// arch::uaccess::copy_to_user.
pub fn copy_stat_to_user(buf: *mut u8, stat: &Stat) -> usize {
    #[cfg(feature = "x86_64")]
    {
        let x = X86Stat::from(stat);
        // SAFETY: caller validated buf with access_ok over
        // user_stat_size() bytes; exception-table copy faults safely.
        unsafe {
            crate::arch::uaccess::copy_to_user(
                buf,
                &x as *const X86Stat as *const u8,
                core::mem::size_of::<X86Stat>(),
            )
        }
    }
    #[cfg(not(feature = "x86_64"))]
    // SAFETY: see above.
    unsafe {
        crate::arch::uaccess::copy_to_user(
            buf,
            stat as *const Stat as *const u8,
            core::mem::size_of::<Stat>(),
        )
    }
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
            stx_mnt_id: 0,
            __spare2: [0; 13],
        }
    }
}

impl Default for Statx {
    fn default() -> Self {
        Self::new()
    }
}

