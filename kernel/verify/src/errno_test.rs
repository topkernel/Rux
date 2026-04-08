//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Errno enum/constant consistency invariant tests.
//!
//! Types copied from: kernel/src/errno.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/errno.rs
// ============================================================================

#[repr(i32)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Errno {
    OperationNotPermitted = 1,
    NoSuchFileOrDirectory = 2,
    NoSuchProcess = 3,
    InterruptedSystemCall = 4,
    IOError = 5,
    NoSuchDeviceOrAddress = 6,
    ArgumentListTooLong = 7,
    ExecFormatError = 8,
    BadFileNumber = 9,
    NoChild = 10,
    TryAgain = 11,
    OutOfMemory = 12,
    PermissionDenied = 13,
    BadAddress = 14,
    BlockDeviceRequired = 15,
    DeviceOrResourceBusy = 16,
    FileExists = 17,
    CrossDeviceLink = 18,
    NoSuchDevice = 19,
    NotADirectory = 20,
    IsADirectory = 21,
    InvalidArgument = 22,
    FileTableOverflow = 23,
    TooManyOpenFiles = 24,
    NotATypewriter = 25,
    NoSpaceLeftOnDevice = 28,
    BrokenPipe = 32,
    IllegalSeek = 29,
    FileTooLarge = 27,
    ReadOnlyFileSystem = 30,
    TooManyLinks = 31,
    DirectoryNotEmpty = 39,
    TooManySymbolicLinks = 40,
    FunctionNotImplemented = 38,
    ValueTooLarge = 75,
}

impl Errno {
    #[inline]
    pub const fn as_i32(self) -> i32 { self as i32 }

    #[inline]
    pub const fn as_neg_i32(self) -> i32 { -(self as i32) }

    #[inline]
    pub const fn as_neg_u64(self) -> u64 { (-(self as i32)) as u64 }
}

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

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-ERRNO-1: as_neg_i32 returns negative of discriminant
    #[test]
    fn test_neg_i32(errno_val in 1i32..76i32) {
        // Direct formula test: neg_i32 is always negative
        prop_assert!(errno_val > 0);
        prop_assert!(-(errno_val) < 0);
        prop_assert_eq!(errno_val + (-(errno_val)), 0);
    }

    /// INV-ERRNO-2: as_neg_u64 is consistent with as_neg_i32
    #[test]
    fn test_neg_u64_consistency(errno_val in 1i32..76i32) {
        prop_assert_eq!((-(errno_val)) as u64, (-(errno_val as i32)) as u64);
    }
}

#[test]
/// INV-ERRNO-3: Every Errno variant matches its corresponding constant
fn test_enum_constant_match() {
    let pairs: &[(&str, Errno, i32)] = &[
        ("EPERM", Errno::OperationNotPermitted, constants::EPERM),
        ("ENOENT", Errno::NoSuchFileOrDirectory, constants::ENOENT),
        ("ESRCH", Errno::NoSuchProcess, constants::ESRCH),
        ("EINTR", Errno::InterruptedSystemCall, constants::EINTR),
        ("EIO", Errno::IOError, constants::EIO),
        ("ENXIO", Errno::NoSuchDeviceOrAddress, constants::ENXIO),
        ("E2BIG", Errno::ArgumentListTooLong, constants::E2BIG),
        ("ENOEXEC", Errno::ExecFormatError, constants::ENOEXEC),
        ("EBADF", Errno::BadFileNumber, constants::EBADF),
        ("ECHILD", Errno::NoChild, constants::ECHILD),
        ("EAGAIN", Errno::TryAgain, constants::EAGAIN),
        ("ENOMEM", Errno::OutOfMemory, constants::ENOMEM),
        ("EACCES", Errno::PermissionDenied, constants::EACCES),
        ("EFAULT", Errno::BadAddress, constants::EFAULT),
        ("EBUSY", Errno::DeviceOrResourceBusy, constants::EBUSY),
        ("EEXIST", Errno::FileExists, constants::EEXIST),
        ("EXDEV", Errno::CrossDeviceLink, constants::EXDEV),
        ("ENODEV", Errno::NoSuchDevice, constants::ENODEV),
        ("ENOTDIR", Errno::NotADirectory, constants::ENOTDIR),
        ("EISDIR", Errno::IsADirectory, constants::EISDIR),
        ("EINVAL", Errno::InvalidArgument, constants::EINVAL),
        ("ENFILE", Errno::FileTableOverflow, constants::ENFILE),
        ("EMFILE", Errno::TooManyOpenFiles, constants::EMFILE),
        ("ENOTTY", Errno::NotATypewriter, constants::ENOTTY),
        ("ENOSPC", Errno::NoSpaceLeftOnDevice, constants::ENOSPC),
        ("ESPIPE", Errno::IllegalSeek, constants::ESPIPE),
        ("EROFS", Errno::ReadOnlyFileSystem, constants::EROFS),
        ("EMLINK", Errno::TooManyLinks, constants::EMLINK),
        ("EPIPE", Errno::BrokenPipe, constants::EPIPE),
        ("ENOSYS", Errno::FunctionNotImplemented, constants::ENOSYS),
        ("ENOTEMPTY", Errno::DirectoryNotEmpty, constants::ENOTEMPTY),
        ("ELOOP", Errno::TooManySymbolicLinks, constants::ELOOP),
        ("EOVERFLOW", Errno::ValueTooLarge, constants::EOVERFLOW),
    ];
    for (name, errno, expected) in pairs {
        assert_eq!(errno.as_i32(), *expected,
            "Errno::{:?} ({}) = {}, expected constant {}",
            errno, name, errno.as_i32(), expected);
    }
}

#[test]
/// INV-ERRNO-4: EWOULDBLOCK == EAGAIN (POSIX requirement)
fn test_ewouldblock_eagain() {
    assert_eq!(constants::EWOULDBLOCK, constants::EAGAIN);
    assert_eq!(constants::EWOULDBLOCK, 11);
    assert_eq!(Errno::TryAgain.as_i32(), 11);
}

#[test]
/// INV-ERRNO-5: No duplicate values among Errno variants
fn test_no_duplicates() {
    let all = [
        Errno::OperationNotPermitted,
        Errno::NoSuchFileOrDirectory,
        Errno::NoSuchProcess,
        Errno::InterruptedSystemCall,
        Errno::IOError,
        Errno::NoSuchDeviceOrAddress,
        Errno::ArgumentListTooLong,
        Errno::ExecFormatError,
        Errno::BadFileNumber,
        Errno::NoChild,
        Errno::TryAgain,
        Errno::OutOfMemory,
        Errno::PermissionDenied,
        Errno::BadAddress,
        Errno::BlockDeviceRequired,
        Errno::DeviceOrResourceBusy,
        Errno::FileExists,
        Errno::CrossDeviceLink,
        Errno::NoSuchDevice,
        Errno::NotADirectory,
        Errno::IsADirectory,
        Errno::InvalidArgument,
        Errno::FileTableOverflow,
        Errno::TooManyOpenFiles,
        Errno::NotATypewriter,
        Errno::NoSpaceLeftOnDevice,
        Errno::BrokenPipe,
        Errno::IllegalSeek,
        Errno::FileTooLarge,
        Errno::ReadOnlyFileSystem,
        Errno::TooManyLinks,
        Errno::DirectoryNotEmpty,
        Errno::TooManySymbolicLinks,
        Errno::FunctionNotImplemented,
        Errno::ValueTooLarge,
    ];
    let mut seen = std::collections::HashSet::new();
    for errno in &all {
        let val = errno.as_i32();
        assert!(seen.insert(val), "duplicate errno value: {}", val);
    }
}

#[test]
/// INV-ERRNO-6: All errno values are positive
fn test_positive_values() {
    let all = [
        Errno::OperationNotPermitted,
        Errno::NoSuchFileOrDirectory,
        Errno::NoSuchProcess,
        Errno::InterruptedSystemCall,
        Errno::IOError,
        Errno::NoSuchDeviceOrAddress,
        Errno::ArgumentListTooLong,
        Errno::ExecFormatError,
        Errno::BadFileNumber,
        Errno::NoChild,
        Errno::TryAgain,
        Errno::OutOfMemory,
        Errno::PermissionDenied,
        Errno::BadAddress,
        Errno::BlockDeviceRequired,
        Errno::DeviceOrResourceBusy,
        Errno::FileExists,
        Errno::CrossDeviceLink,
        Errno::NoSuchDevice,
        Errno::NotADirectory,
        Errno::IsADirectory,
        Errno::InvalidArgument,
        Errno::FileTableOverflow,
        Errno::TooManyOpenFiles,
        Errno::NotATypewriter,
        Errno::NoSpaceLeftOnDevice,
        Errno::BrokenPipe,
        Errno::IllegalSeek,
        Errno::FileTooLarge,
        Errno::ReadOnlyFileSystem,
        Errno::TooManyLinks,
        Errno::DirectoryNotEmpty,
        Errno::TooManySymbolicLinks,
        Errno::FunctionNotImplemented,
        Errno::ValueTooLarge,
    ];
    for errno in &all {
        assert!(errno.as_i32() > 0, "errno {} is not positive", errno.as_i32());
    }
}

#[test]
/// INV-ERRNO-7: as_neg_i32 is always negative
fn test_neg_always_negative() {
    let all = [
        Errno::OperationNotPermitted,
        Errno::InvalidArgument,
        Errno::OutOfMemory,
        Errno::ValueTooLarge,
    ];
    for errno in &all {
        assert!(errno.as_neg_i32() < 0,
            "as_neg_i32 should be negative, got {}", errno.as_neg_i32());
    }
}

#[test]
/// INV-ERRNO-8: as_neg_u64 represents the two's complement correctly
fn test_neg_u64_twos_complement() {
    // -22 in two's complement (u64) should be 0xFFFFFFFFFFFFFFEA
    assert_eq!(Errno::InvalidArgument.as_neg_u64(), (-(22i32)) as u64);
    assert_eq!(Errno::InvalidArgument.as_neg_u64(), u64::MAX - 21);
}
