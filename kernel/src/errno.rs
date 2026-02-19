//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! 标准错误代码定义
//!
//! 和 include/uapi/asm-generic/errno.h

/// 标准错误代码
///
///
/// 使用方法：
/// ```rust
/// use crate::errno;
///
/// // 返回错误（系统调用风格，返回负数）
/// return Err(errno::ENOENT as i32);
///
/// // 或者使用 Errno 枚举
/// return Err(errno::Errno::NoSuchFileOrDirectory.as_neg_i32());
/// ```
#[repr(i32)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Errno {
    /// Operation not permitted (EPERM, 1)
    OperationNotPermitted = 1,

    /// No such file or directory (ENOENT, 2)
    NoSuchFileOrDirectory = 2,

    /// No such process (ESRCH, 3)
    NoSuchProcess = 3,

    /// Interrupted system call (EINTR, 4)
    InterruptedSystemCall = 4,

    /// I/O error (EIO, 5)
    IOError = 5,

    /// No such device or address (ENXIO, 6)
    NoSuchDeviceOrAddress = 6,

    /// Argument list too long (E2BIG, 7)
    ArgumentListTooLong = 7,

    /// Exec format error (ENOEXEC, 8)
    ExecFormatError = 8,

    /// Bad file number (EBADF, 9)
    BadFileNumber = 9,

    /// No child process (ECHILD, 10)
    NoChild = 10,

    /// Try again (EAGAIN, 11)
    TryAgain = 11,

    /// Out of memory (ENOMEM, 12)
    OutOfMemory = 12,

    /// Permission denied (EACCES, 13)
    PermissionDenied = 13,

    /// Bad address (EFAULT, 14)
    BadAddress = 14,

    /// Block device required (EBLKREQ, 15)
    BlockDeviceRequired = 15,

    /// Device or resource busy (EBUSY, 16)
    DeviceOrResourceBusy = 16,

    /// File exists (EEXIST, 17)
    FileExists = 17,

    /// Cross-device link (EXDEV, 18)
    CrossDeviceLink = 18,

    /// No such device (ENODEV, 19)
    NoSuchDevice = 19,

    /// Not a directory (ENOTDIR, 20)
    NotADirectory = 20,

    /// Is a directory (EISDIR, 21)
    IsADirectory = 21,

    /// Invalid argument (EINVAL, 22)
    InvalidArgument = 22,

    /// File table overflow (ENFILE, 23)
    FileTableOverflow = 23,

    /// Too many open files (EMFILE, 24)
    TooManyOpenFiles = 24,

    /// Not a typewriter (ENOTTY, 25)
    NotATypewriter = 25,

    /// No space left on device (ENOSPC, 28)
    NoSpaceLeftOnDevice = 28,

    /// Illegal seek (ESPIPE, 29)
    IllegalSeek = 29,

    /// File too large (EFBIG, 27)
    FileTooLarge = 27,

    /// Read-only file system (EROFS, 30)
    ReadOnlyFileSystem = 30,

    /// Too many links (EMLINK, 31)
    TooManyLinks = 31,

    /// Broken pipe (EPIPE, 32)
    BrokenPipe = 32,

    /// Directory not empty (ENOTEMPTY, 39)
    DirectoryNotEmpty = 39,

    /// Function not implemented (ENOSYS, 38)
    FunctionNotImplemented = 38,

    /// Value too large (EOVERFLOW, 75)
    ValueTooLarge = 75,
}

impl Errno {
    /// 获取错误代码的正数值（用于比较）
    #[inline]
    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    /// 获取错误代码的负数值（用于系统调用返回）
    #[inline]
    pub const fn as_neg_i32(self) -> i32 {
        -(self as i32)
    }

    /// 获取错误代码的负数值（u64，用于系统调用返回）
    #[inline]
    pub const fn as_neg_u64(self) -> u64 {
        (-(self as i32)) as u64
    }
}

/// 常用的错误代码常量
///
/// ...
pub mod constants {
    pub const EPERM: i32 = 1;
    pub const ENOENT: i32 = 2;
    pub const ESRCH: i32 = 3;
    pub const EINTR: i32 = 4;
    pub const EIO: i32 = 5;
    pub const ENXIO: i32 = 6;
    pub const E2BIG: i32 = 7;
    pub const ENOEXEC: i32 = 8;
    pub const EBADF: i32 = 9;
    pub const ECHILD: i32 = 10;
    pub const EAGAIN: i32 = 11;
    pub const ENOMEM: i32 = 12;
    pub const EACCES: i32 = 13;
    pub const EFAULT: i32 = 14;
    pub const EBUSY: i32 = 16;
    pub const EEXIST: i32 = 17;
    pub const EXDEV: i32 = 18;
    pub const ENODEV: i32 = 19;
    pub const ENOTDIR: i32 = 20;
    pub const EISDIR: i32 = 21;
    pub const EINVAL: i32 = 22;
    pub const ENFILE: i32 = 23;
    pub const EMFILE: i32 = 24;
    pub const ENOTTY: i32 = 25;
    pub const ENOSPC: i32 = 28;
    pub const ESPIPE: i32 = 29;
    pub const EROFS: i32 = 30;
    pub const EMLINK: i32 = 31;
    pub const EPIPE: i32 = 32;
    pub const EDOM: i32 = 33;
    pub const ERANGE: i32 = 34;
    pub const EDEADLK: i32 = 35;
    pub const ENAMETOOLONG: i32 = 36;
    pub const ENOLCK: i32 = 37;
    pub const ENOSYS: i32 = 38;
    pub const ENOTEMPTY: i32 = 39;
    pub const ELOOP: i32 = 40;
    pub const EWOULDBLOCK: i32 = 11;
    pub const ENOMSG: i32 = 42;
    pub const EOVERFLOW: i32 = 75;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_errno_values() {
        assert_eq!(Errno::NoSuchFileOrDirectory.as_i32(), 2);
        assert_eq!(Errno::BadFileNumber.as_i32(), 9);
        assert_eq!(Errno::InvalidArgument.as_i32(), 22);
        assert_eq!(Errno::PermissionDenied.as_i32(), 13);
    }

    #[test]
    fn test_errno_negative() {
        assert_eq!(Errno::NoSuchFileOrDirectory.as_neg_i32(), -2);
        assert_eq!(Errno::BadFileNumber.as_neg_i32(), -9);
        assert_eq!(Errno::InvalidArgument.as_neg_i32(), -22);
    }

    #[test]
    fn test_errno_constants() {
        assert_eq!(constants::ENOENT, 2);
        assert_eq!(constants::EBADF, 9);
        assert_eq!(constants::EINVAL, 22);
    }
}
