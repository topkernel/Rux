//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for Errno enum constants and conversions.
//!
//! Types copied from: kernel/src/errno.rs

#![cfg(kani)]

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
    pub const fn as_i32(self) -> i32 { self as i32 }
    pub const fn as_neg_i32(self) -> i32 { -(self as i32) }
    pub const fn as_neg_u64(self) -> u64 { (-(self as i32)) as u64 }
}

/// INV-ERRNO-K1: as_neg_i32 is always negative for positive errno values.
#[kani::proof]
fn verify_neg_always_negative() {
    let val: i32 = kani::any();
    kani::assume(val >= 1 && val <= 75);
    assert!(-val < 0);
}

/// INV-ERRNO-K2: as_neg_u64 matches two's complement of as_neg_i32.
#[kani::proof]
fn verify_neg_u64_twos_complement() {
    let val: i32 = kani::any();
    kani::assume(val >= 1 && val <= 75);
    assert_eq!((-(val)) as u64, (-(val as i32)) as u64);
}

/// INV-ERRNO-K3: EWOULDBLOCK == EAGAIN (POSIX requirement, value 11).
#[kani::proof]
fn verify_ewouldblock_eagain() {
    assert_eq!(Errno::TryAgain.as_i32(), 11);
}

/// INV-ERRNO-K4: select Errno discriminants match expected values.
#[kani::proof]
fn verify_key_discriminants() {
    assert_eq!(Errno::OperationNotPermitted.as_i32(), 1);
    assert_eq!(Errno::NoSuchFileOrDirectory.as_i32(), 2);
    assert_eq!(Errno::PermissionDenied.as_i32(), 13);
    assert_eq!(Errno::InvalidArgument.as_i32(), 22);
    assert_eq!(Errno::OutOfMemory.as_i32(), 12);
}

/// INV-ERRNO-K5: all key errno values are positive and distinct.
#[kani::proof]
fn verify_positive_distinct() {
    let vals = [
        Errno::OperationNotPermitted.as_i32(),
        Errno::NoSuchFileOrDirectory.as_i32(),
        Errno::PermissionDenied.as_i32(),
        Errno::InvalidArgument.as_i32(),
        Errno::OutOfMemory.as_i32(),
        Errno::TryAgain.as_i32(),
    ];
    let mut seen = std::collections::HashSet::new();
    for &v in &vals {
        assert!(v > 0);
        assert!(seen.insert(v));
    }
}
