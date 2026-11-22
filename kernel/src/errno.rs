//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Standard error code definitions

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

/// Standard error codes
///
///
/// Usage:
/// ```rust
/// use crate::errno;
///
/// // Return error (system call style, return negative number)
/// return Err(errno::ENOENT as i32);
///
/// // Or use Errno enum
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

    /// Text file busy (ETXTBSY, 26)
    TextFileBusy = 26,

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

    /// Resource deadlock avoided (EDEADLK, 35)
    Deadlock = 35,

    /// Result too large / buffer too small for the xattr (ERANGE, 34)
    ResultTooLarge = 34,

    /// No data available (ENODATA, 61)
    NoData = 61,

    /// Operation not supported (EOPNOTSUPP, 95)
    OperationNotSupported = 95,

    /// Directory not empty (ENOTEMPTY, 39)
    DirectoryNotEmpty = 39,

    /// Too many symbolic links (ELOOP, 40)
    TooManySymbolicLinks = 40,

    /// Function not implemented (ENOSYS, 38)
    FunctionNotImplemented = 38,

    /// Value too large (EOVERFLOW, 75)
    ValueTooLarge = 75,
}

impl Errno {
    /// Get positive value of error code (for comparison)
    #[inline]
    pub const fn as_i32(self) -> i32 {
        self as i32
    }

    /// Get negative value of error code (for system call return)
    #[inline]
    pub const fn as_neg_i32(self) -> i32 {
        -(self as i32)
    }

    /// MIGRATION BRIDGE (divergence D-10, §5): rebuild an `Errno` from
    /// the ABI's negated form, for call sites where a CONVERTED
    /// function calls into a not-yet-converted internal layer (the
    /// ext4 namei/inode paths, the sched cgroup helpers) via `?`.
    /// Prefer converting the layer — every `?` that resolves through
    /// this impl is a marker to retire. Codes outside the enum map to
    /// `IOError` (documented fallback; no in-tree producer emits a code
    /// the enum does not name).
    #[inline]
    pub const fn from_neg_i32(v: i32) -> Self {
        match -v {
            1 => Self::OperationNotPermitted,
            2 => Self::NoSuchFileOrDirectory,
            3 => Self::NoSuchProcess,
            4 => Self::InterruptedSystemCall,
            5 => Self::IOError,
            6 => Self::NoSuchDeviceOrAddress,
            7 => Self::ArgumentListTooLong,
            8 => Self::ExecFormatError,
            9 => Self::BadFileNumber,
            10 => Self::NoChild,
            11 => Self::TryAgain,
            12 => Self::OutOfMemory,
            13 => Self::PermissionDenied,
            14 => Self::BadAddress,
            15 => Self::BlockDeviceRequired,
            16 => Self::DeviceOrResourceBusy,
            17 => Self::FileExists,
            18 => Self::CrossDeviceLink,
            19 => Self::NoSuchDevice,
            20 => Self::NotADirectory,
            21 => Self::IsADirectory,
            22 => Self::InvalidArgument,
            23 => Self::FileTableOverflow,
            24 => Self::TooManyOpenFiles,
            25 => Self::NotATypewriter,
            26 => Self::TextFileBusy,
            28 => Self::NoSpaceLeftOnDevice,
            29 => Self::IllegalSeek,
            27 => Self::FileTooLarge,
            30 => Self::ReadOnlyFileSystem,
            31 => Self::TooManyLinks,
            32 => Self::BrokenPipe,
            35 => Self::Deadlock,
            34 => Self::ResultTooLarge,
            61 => Self::NoData,
            95 => Self::OperationNotSupported,
            39 => Self::DirectoryNotEmpty,
            40 => Self::TooManySymbolicLinks,
            38 => Self::FunctionNotImplemented,
            75 => Self::ValueTooLarge,
            _ => Self::IOError,
        }
    }

    /// Get negative value of error code (u64, for system call return)
    #[inline]
    pub const fn as_neg_u64(self) -> u64 {
        (-(self as i32)) as u64
    }
}

/// Common error code constants
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
    /// Operation not supported (EOPNOTSUPP/ENOTSUP, 95) — used by the
    /// U2 minimal stubs (name_to_handle_at, process_vm_*) so callers can
    /// distinguish "unsupported here" from "syscall absent" (ENOSYS).
    pub const EOPNOTSUPP: i32 = 95;
    // Extended asm-generic set (unified 2026-11-21 from syscall::errno:
    // the framework errno table is the single home; syscall re-exports).
    pub const ENOTBLK: i32 = 15;
    pub const ETXTBSY: i32 = 26;
    pub const EFBIG: i32 = 27;
    pub const EIDRM: i32 = 43;
    pub const ECHRNG: i32 = 44;
    pub const EL2NSYNC: i32 = 45;
    pub const EL3HLT: i32 = 46;
    pub const EL3RST: i32 = 47;
    pub const ELNRNG: i32 = 48;
    pub const EUNATCH: i32 = 49;
    pub const ENOCSI: i32 = 50;
    pub const EL2HLT: i32 = 51;
    pub const EBADE: i32 = 52;
    pub const EBADR: i32 = 53;
    pub const EXFULL: i32 = 54;
    pub const ENOANO: i32 = 55;
    pub const EBADRQC: i32 = 56;
    pub const EBADSLT: i32 = 57;
    pub const EBFONT: i32 = 59;
    pub const ENOSTR: i32 = 60;
    pub const ENODATA: i32 = 61;
    pub const ETIME: i32 = 62;
    pub const ENOSR: i32 = 63;
    pub const ENONET: i32 = 64;
    pub const ENOPKG: i32 = 65;
    pub const EREMOTE: i32 = 66;
    pub const ENOLINK: i32 = 67;
    pub const EADV: i32 = 68;
    pub const ESRMNT: i32 = 69;
    pub const ECOMM: i32 = 70;
    pub const EPROTO: i32 = 71;
    pub const EMULTIHOP: i32 = 72;
    pub const EDOTDOT: i32 = 73;
    pub const EBADMSG: i32 = 74;
    pub const ENOTUNIQ: i32 = 76;
    pub const EBADFD: i32 = 77;
    pub const EREMCHG: i32 = 78;
    pub const ELIBACC: i32 = 79;
    pub const ELIBBAD: i32 = 80;
    pub const ELIBSCN: i32 = 81;
    pub const ELIBMAX: i32 = 82;
    pub const ELIBEXEC: i32 = 83;
    pub const EILSEQ: i32 = 84;
    pub const ERESTART: i32 = 85;
    pub const ESTRPIPE: i32 = 86;
    pub const EUSERS: i32 = 87;
    pub const ENOTSOCK: i32 = 88;
    pub const EDESTADDRREQ: i32 = 89;
    pub const EMSGSIZE: i32 = 90;
    pub const EPROTOTYPE: i32 = 91;
    pub const ENOPROTOOPT: i32 = 92;
    pub const EPROTONOSUPPORT: i32 = 93;
    pub const ESOCKTNOSUPPORT: i32 = 94;
    pub const EPFNOSUPPORT: i32 = 96;
    pub const EAFNOSUPPORT: i32 = 97;
    pub const EADDRINUSE: i32 = 98;
    pub const EADDRNOTAVAIL: i32 = 99;
    pub const ENETDOWN: i32 = 100;
    pub const ENETUNREACH: i32 = 101;
    pub const ENETRESET: i32 = 102;
    pub const ECONNABORTED: i32 = 103;
    pub const ECONNRESET: i32 = 104;
    pub const ENOBUFS: i32 = 105;
    pub const EISCONN: i32 = 106;
    pub const ENOTCONN: i32 = 107;
    pub const ESHUTDOWN: i32 = 108;
    pub const ETOOMANYREFS: i32 = 109;
    pub const ETIMEDOUT: i32 = 110;
    pub const ECONNREFUSED: i32 = 111;
    pub const EHOSTDOWN: i32 = 112;
    pub const EHOSTUNREACH: i32 = 113;
    pub const EALREADY: i32 = 114;
    pub const EINPROGRESS: i32 = 115;
    pub const ESTALE: i32 = 116;
    pub const EUCLEAN: i32 = 117;
    pub const ENOTNAM: i32 = 118;
    pub const ENAVAIL: i32 = 119;
    pub const EISNAM: i32 = 120;
    pub const EREMOTEIO: i32 = 121;
    pub const EDQUOT: i32 = 122;
    pub const ENOMEDIUM: i32 = 123;
    pub const EMEDIUMTYPE: i32 = 124;
    pub const ECANCELED: i32 = 125;
    pub const ENOKEY: i32 = 126;
    pub const EKEYEXPIRED: i32 = 127;
    pub const EKEYREVOKED: i32 = 128;
    pub const EKEYREJECTED: i32 = 129;
    pub const EOWNERDEAD: i32 = 130;
    pub const ENOTRECOVERABLE: i32 = 131;
    pub const ERFKILL: i32 = 132;
    pub const EHWPOISON: i32 = 133;
}


/// MIGRATION BRIDGE (see `Errno::from_neg_i32`): lets `?` in a
/// Result<_, Errno> function absorb a Result<_, i32> from an
/// unconverted layer. Retire with the deep-layer conversions.
impl From<i32> for Errno {
    #[inline]
    fn from(v: i32) -> Self {
        Self::from_neg_i32(v)
    }
}
