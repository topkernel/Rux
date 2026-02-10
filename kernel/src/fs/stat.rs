//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 文件状态信息 (stat)
//!
//! 对应 Linux 的 stat 结构体 (include/uapi/asm-generic/stat.h)

/// 文件状态信息
///
/// 对应 Linux 的 `struct stat64` (64位系统)
///
/// 参考 Linux 内核: include/uapi/asm-generic/stat.h
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Stat {
    /// 设备 ID (st_dev)
    /// 如果是文件，则包含文件所在的设备 ID
    pub st_dev: u64,

    /// Inode 号 (st_ino)
    pub st_ino: u64,

    /// 文件类型和权限 (st_mode)
    ///
    /// 位掩码:
    /// - S_IFMT  (0o170000) - 文件类型掩码
    /// - S_IFREG (0o100000) - 常规文件
    /// - S_IFDIR (0o040000) - 目录
    /// - S_IFCHR (0o020000) - 字符设备
    /// - S_IFBLK (0o060000) - 块设备
    /// - S_IFIFO (0o010000) - FIFO
    /// - S_IFLNK (0o120000) - 符号链接
    /// - S_IFSOCK(0o140000) - socket
    /// - 权限位: 0o777 (rwxrwxrwx)
    pub st_mode: u32,

    /// 硬链接数 (st_nlink)
    pub st_nlink: u32,

    /// 用户 ID (st_uid)
    pub st_uid: u32,

    /// 组 ID (st_gid)
    pub st_gid: u32,

    /// 设备 ID（如果是特殊文件）(st_rdev)
    pub st_rdev: u64,

    /// 文件大小 (字节) (st_size)
    pub st_size: i64,

    /// 块大小 (st_blksize)
    /// 文件系统 I/O 的首选块大小
    pub st_blksize: u64,

    /// 分配的 512字节块数 (st_blocks)
    pub st_blocks: u64,

    /// 最后访问时间 (st_atime)
    pub st_atime: u64,

    /// 最后访问时间的纳秒部分 (st_atime_nsec)
    pub st_atime_nsec: u64,

    /// 最后修改时间 (st_mtime)
    pub st_mtime: u64,

    /// 最后修改时间的纳秒部分 (st_mtime_nsec)
    pub st_mtime_nsec: u64,

    /// 最后状态改变时间 (st_ctime)
    pub st_ctime: u64,

    /// 最后状态改变时间的纳秒部分 (st_ctime_nsec)
    pub st_ctime_nsec: u64,
}

impl Stat {
    /// 创建默认的 Stat 结构
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
            st_blksize: 4096,  // 默认 4KB
            st_blocks: 0,
            st_atime: 0,
            st_atime_nsec: 0,
            st_mtime: 0,
            st_mtime_nsec: 0,
            st_ctime: 0,
            st_ctime_nsec: 0,
        }
    }

    /// 设置为常规文件
    pub fn set_regular_file(&mut self) {
        self.st_mode = (self.st_mode & !0o170000) | 0o100000;
    }

    /// 设置为目录
    pub fn set_directory(&mut self) {
        self.st_mode = (self.st_mode & !0o170000) | 0o040000;
    }

    /// 设置为字符设备
    pub fn set_char_device(&mut self) {
        self.st_mode = (self.st_mode & !0o170000) | 0o020000;
    }

    /// 设置为块设备
    pub fn set_block_device(&mut self) {
        self.st_mode = (self.st_mode & !0o170000) | 0o060000;
    }

    /// 设置为 FIFO
    pub fn set_fifo(&mut self) {
        self.st_mode = (self.st_mode & !0o170000) | 0o010000;
    }

    /// 设置为符号链接
    pub fn set_symlink(&mut self) {
        self.st_mode = (self.st_mode & !0o170000) | 0o120000;
    }

    /// 设置为 socket
    pub fn set_socket(&mut self) {
        self.st_mode = (self.st_mode & !0o170000) | 0o140000;
    }

    /// 检查是否是常规文件
    pub fn is_regular_file(&self) -> bool {
        (self.st_mode & 0o170000) == 0o100000
    }

    /// 检查是否是目录
    pub fn is_directory(&self) -> bool {
        (self.st_mode & 0o170000) == 0o040000
    }

    /// 检查是否是字符设备
    pub fn is_char_device(&self) -> bool {
        (self.st_mode & 0o170000) == 0o020000
    }

    /// 检查是否是块设备
    pub fn is_block_device(&self) -> bool {
        (self.st_mode & 0o170000) == 0o060000
    }

    /// 检查是否是 FIFO
    pub fn is_fifo(&self) -> bool {
        (self.st_mode & 0o170000) == 0o010000
    }

    /// 检查是否是符号链接
    pub fn is_symlink(&self) -> bool {
        (self.st_mode & 0o170000) == 0o120000
    }

    /// 检查是否是 socket
    pub fn is_socket(&self) -> bool {
        (self.st_mode & 0o170000) == 0o140000
    }

    /// 设置权限位
    pub fn set_mode(&mut self, mode: u32) {
        // 清除低 9 位的权限
        self.st_mode &= 0o170000;
        // 设置新的权限
        self.st_mode |= mode & 0o777;
    }

    /// 获取权限位
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
