//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for PageFlags bit operations.
//!
//! Types copied from: kernel/verify/src/mm/page_flags_ops_test.rs

#![cfg(kani)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PageFlag {
    Locked      = 1 << 0,
    Writeback   = 1 << 1,
    Referenced  = 1 << 2,
    UpToDate    = 1 << 3,
    Dirty       = 1 << 4,
    Lru         = 1 << 5,
    Head        = 1 << 6,
    Waiters     = 1 << 7,
    Active      = 1 << 8,
    Reserved    = 1 << 9,
    Private     = 1 << 10,
    Reclaim     = 1 << 11,
    SwapBacked  = 1 << 12,
    Unevictable = 1 << 13,
    Cow         = 1 << 14,
    Anonymous   = 1 << 15,
}

const ALL_FLAGS: [PageFlag; 16] = [
    PageFlag::Locked, PageFlag::Writeback, PageFlag::Referenced, PageFlag::UpToDate,
    PageFlag::Dirty, PageFlag::Lru, PageFlag::Head, PageFlag::Waiters,
    PageFlag::Active, PageFlag::Reserved, PageFlag::Private, PageFlag::Reclaim,
    PageFlag::SwapBacked, PageFlag::Unevictable, PageFlag::Cow, PageFlag::Anonymous,
];

#[derive(Debug)]
pub struct PageFlags(u32);

impl PageFlags {
    pub const fn new() -> Self { Self(0) }
    pub const fn from_raw(flags: u32) -> Self { Self(flags) }
    pub fn raw(&self) -> u32 { self.0 }
    pub fn test(&self, flag: PageFlag) -> bool { self.0 & (flag as u32) != 0 }
    pub fn set(&mut self, flag: PageFlag) { self.0 |= flag as u32; }
    pub fn clear(&mut self, flag: PageFlag) { self.0 &= !(flag as u32); }
    pub fn clear_all(&mut self) { self.0 = 0; }
}

/// INV-PGFL-K1: set → test returns true → clear → test returns false
#[kani::proof]
fn verify_flags_set_test_clear_roundtrip() {
    let idx: usize = kani::any();
    kani::assume(idx < 16);
    let flag = ALL_FLAGS[idx];
    let mut flags = PageFlags::new();
    flags.set(flag);
    assert!(flags.test(flag));
    flags.clear(flag);
    assert!(!flags.test(flag));
}

/// INV-PGFL-K2: from_raw(raw).raw() == raw for all u32
#[kani::proof]
fn verify_flags_from_raw_roundtrip() {
    let raw: u32 = kani::any();
    let flags = PageFlags::from_raw(raw);
    assert_eq!(flags.raw(), raw);
}

/// INV-PGFL-K3: All 16 PageFlag values are distinct powers of 2
#[kani::proof]
fn verify_flags_no_overlap() {
    let mut seen: u32 = 0;
    for flag in ALL_FLAGS {
        let val = flag as u32;
        assert!(val != 0);
        assert_eq!(val & (val - 1), 0);  // power of 2
        assert_eq!(seen & val, 0);       // distinct
        seen |= val;
    }
}

/// INV-PGFL-K4: clear_all → raw() == 0 for any initial state
#[kani::proof]
fn verify_flags_clear_all() {
    let raw: u32 = kani::any();
    let mut flags = PageFlags::from_raw(raw);
    flags.clear_all();
    assert_eq!(flags.raw(), 0);
}

/// INV-PGFL-K5: set(flag) twice is idempotent
#[kani::proof]
fn verify_flags_set_idempotent() {
    let idx: usize = kani::any();
    kani::assume(idx < 16);
    let flag = ALL_FLAGS[idx];
    let mut flags = PageFlags::new();
    flags.set(flag);
    let raw1 = flags.raw();
    flags.set(flag);
    let raw2 = flags.raw();
    assert_eq!(raw1, raw2);
}
