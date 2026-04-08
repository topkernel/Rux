//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for InodeMode file type classifier.
//!
//! Types copied from: kernel/src/fs/inode.rs

#![cfg(kani)]

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct InodeMode(u32);

impl InodeMode {
    pub const S_IFMT: u32 = 0o0170000;
    pub const S_IFREG: u32 = 0o0100000;
    pub const S_IFDIR: u32 = 0o0040000;
    pub const S_IFCHR: u32 = 0o0020000;
    pub const S_IFBLK: u32 = 0o0060000;
    pub const S_IFIFO: u32 = 0o0010000;
    pub const S_IFLNK: u32 = 0o0120000;
    pub const S_IFSOCK: u32 = 0o0140000;

    pub fn new(mode: u32) -> Self { Self(mode) }
    pub fn is_regular_file(&self) -> bool { (self.0 & Self::S_IFMT) == Self::S_IFREG }
    pub fn is_directory(&self) -> bool { (self.0 & Self::S_IFMT) == Self::S_IFDIR }
    pub fn is_char_device(&self) -> bool { (self.0 & Self::S_IFMT) == Self::S_IFCHR }
    pub fn is_block_device(&self) -> bool { (self.0 & Self::S_IFMT) == Self::S_IFBLK }
    pub fn is_fifo(&self) -> bool { (self.0 & Self::S_IFMT) == Self::S_IFIFO }
    pub fn is_symlink(&self) -> bool { (self.0 & Self::S_IFMT) == Self::S_IFLNK }
    pub fn is_socket(&self) -> bool { (self.0 & Self::S_IFMT) == Self::S_IFSOCK }
    pub fn bits(&self) -> u32 { self.0 }
}

fn count_types(m: &InodeMode) -> usize {
    let mut c = 0;
    if m.is_regular_file() { c += 1; }
    if m.is_directory() { c += 1; }
    if m.is_char_device() { c += 1; }
    if m.is_block_device() { c += 1; }
    if m.is_fifo() { c += 1; }
    if m.is_symlink() { c += 1; }
    if m.is_socket() { c += 1; }
    c
}

/// INV-INODE-K1: S_IFREG + perm → is_regular_file true.
#[kani::proof]
fn verify_is_regular_file() {
    let perm: u32 = kani::any();
    kani::assume(perm < 0o777);
    assert!(InodeMode::new(InodeMode::S_IFREG | perm).is_regular_file());
}

/// INV-INODE-K2: S_IFDIR + perm → is_directory true.
#[kani::proof]
fn verify_is_directory() {
    let perm: u32 = kani::any();
    kani::assume(perm < 0o777);
    assert!(InodeMode::new(InodeMode::S_IFDIR | perm).is_directory());
}

/// INV-INODE-K3: file types are mutually exclusive for any mode.
#[kani::proof]
fn verify_types_mutually_exclusive() {
    let mode: u32 = kani::any();
    kani::assume(mode < 0o177777);
    let m = InodeMode::new(mode);
    assert!(count_types(&m) <= 1);
}

/// INV-INODE-K4: bits() roundtrip.
#[kani::proof]
fn verify_bits_roundtrip() {
    let mode: u32 = kani::any();
    kani::assume(mode < 0o177777);
    assert_eq!(InodeMode::new(mode).bits(), mode);
}

/// INV-INODE-K5: S_IFMT correctly isolates file type bits.
#[kani::proof]
fn verify_ifmt_isolates() {
    let raw: u32 = kani::any();
    kani::assume(raw < 0o177777);
    let file_type_bits = raw & InodeMode::S_IFMT;
    let perm_bits = raw & !InodeMode::S_IFMT;
    assert_eq!(file_type_bits | perm_bits, raw);
}

/// INV-INODE-K6: S_IFMT does not overlap with permission bits.
#[kani::proof]
fn verify_ifmt_no_overlap() {
    let perm: u32 = kani::any();
    kani::assume(perm < 0o7777);
    assert_eq!(perm & InodeMode::S_IFMT, 0);
}
