//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for Stat file type/permission operations.
//!
//! Types copied from: kernel/src/fs/stat.rs

#![cfg(kani)]

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct Stat { pub st_mode: u32 }

impl Stat {
    pub fn new() -> Self { Self { st_mode: 0 } }
    pub fn set_regular_file(&mut self) { self.st_mode = (self.st_mode & !0o170000) | 0o100000; }
    pub fn set_directory(&mut self) { self.st_mode = (self.st_mode & !0o170000) | 0o040000; }
    pub fn is_regular_file(&self) -> bool { (self.st_mode & 0o170000) == 0o100000 }
    pub fn is_directory(&self) -> bool { (self.st_mode & 0o170000) == 0o040000 }
    pub fn set_mode(&mut self, mode: u32) { self.st_mode &= 0o170000; self.st_mode |= mode & 0o777; }
    pub fn get_mode(&self) -> u32 { self.st_mode & 0o777 }
}

fn count_types(s: &Stat) -> usize {
    let mut count = 0;
    if s.is_regular_file() { count += 1; }
    if s.is_directory() { count += 1; }
    count
}

/// INV-STAT-K1: set_mode/get_mode roundtrip.
#[kani::proof]
fn verify_mode_roundtrip() {
    let mode: u32 = kani::any();
    kani::assume(mode < 0o777);
    let mut s = Stat::new();
    s.set_mode(mode);
    assert_eq!(s.get_mode(), mode);
}

/// INV-STAT-K2: set_mode preserves file type.
#[kani::proof]
fn verify_set_mode_preserves_type() {
    let mode: u32 = kani::any();
    kani::assume(mode < 0o777);
    let mut s = Stat::new();
    s.set_directory();
    s.set_mode(mode);
    assert!(s.is_directory());
}

/// INV-STAT-K3: get_mode returns only low 9 bits.
#[kani::proof]
fn verify_get_mode_low_bits() {
    let raw: u32 = kani::any();
    let s = Stat { st_mode: raw };
    assert_eq!(s.get_mode(), raw & 0o777);
}

/// INV-STAT-K4: file types are mutually exclusive for any raw mode.
#[kani::proof]
fn verify_mutual_exclusivity() {
    let raw: u32 = kani::any();
    let s = Stat { st_mode: raw };
    assert!(count_types(&s) <= 1);
}

/// INV-STAT-K5: set_type overwrites previous type.
#[kani::proof]
fn verify_type_overwrite() {
    let perm: u32 = kani::any();
    kani::assume(perm < 0o777);
    let mut s = Stat::new();
    s.set_mode(perm);
    s.set_regular_file();
    assert!(s.is_regular_file());
    s.set_directory();
    assert!(s.is_directory());
    assert!(!s.is_regular_file());
    assert_eq!(s.get_mode(), perm);
}
