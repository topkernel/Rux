//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for SigPending bitmap and SigAction.
//!
//! Types copied from: kernel/src/signal.rs

#![cfg(kani)]

pub struct SigPending {
    pub signal: u64,
}

impl SigPending {
    pub fn new() -> Self { Self { signal: 0 } }

    pub fn add(&mut self, sig: i32) {
        if sig < 1 || sig > 64 { return; }
        self.signal |= 1u64 << (sig - 1);
    }

    pub fn remove(&mut self, sig: i32) {
        if sig < 1 || sig > 64 { return; }
        self.signal &= !(1u64 << (sig - 1));
    }

    pub fn has(&self, sig: i32) -> bool {
        if sig < 1 || sig > 64 { return false; }
        (self.signal & (1u64 << (sig - 1))) != 0
    }

    pub fn first(&self) -> Option<i32> {
        if self.signal == 0 { return None; }
        Some(self.signal.trailing_zeros() as i32 + 1)
    }

    pub fn first_unmasked(&self, mask: u64) -> Option<i32> {
        let deliverable = self.signal & !mask;
        if deliverable == 0 { return None; }
        Some(deliverable.trailing_zeros() as i32 + 1)
    }

    pub fn clear(&mut self) { self.signal = 0; }
}

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum SigActionKind { Default = 0, Ignore = 1, Handler = 2 }

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct SigAction {
    pub sa_handler: usize,
    pub sa_flags: u32,
    pub sa_mask: u64,
}

impl SigAction {
    fn default_handler() -> usize { SigActionKind::Default as usize }
    fn ignore_handler() -> usize { SigActionKind::Ignore as usize }

    pub fn new() -> Self {
        Self { sa_handler: Self::default_handler(), sa_flags: 0, sa_mask: 0 }
    }

    pub fn action(&self) -> SigActionKind {
        if self.sa_handler == Self::default_handler() { SigActionKind::Default }
        else if self.sa_handler == Self::ignore_handler() { SigActionKind::Ignore }
        else { SigActionKind::Handler }
    }
}

/// INV-SIG-K1: add(sig); has(sig) for sig in 1..=64.
#[kani::proof]
fn verify_add_has() {
    let sig: i32 = kani::any();
    kani::assume(sig >= 1 && sig <= 64);
    let mut p = SigPending::new();
    p.add(sig);
    assert!(p.has(sig));
}

/// INV-SIG-K2: add(sig); remove(sig); !has(sig) roundtrip.
#[kani::proof]
fn verify_add_remove_roundtrip() {
    let sig: i32 = kani::any();
    kani::assume(sig >= 1 && sig <= 64);
    let mut p = SigPending::new();
    p.add(sig);
    p.remove(sig);
    assert!(!p.has(sig));
}

/// INV-SIG-K3: first() returns lowest set bit signal number.
#[kani::proof]
fn verify_first_lowest() {
    let sig1: i32 = kani::any();
    let sig2: i32 = kani::any();
    kani::assume(sig1 >= 1 && sig1 <= 64);
    kani::assume(sig2 >= 1 && sig2 <= 64);
    let mut p = SigPending::new();
    p.add(sig1);
    p.add(sig2);
    let first = p.first().unwrap();
    assert_eq!(first, std::cmp::min(sig1, sig2));
}

/// INV-SIG-K4: first_unmasked(0) == first().
#[kani::proof]
fn verify_first_unmasked_no_mask() {
    let sig1: i32 = kani::any();
    let sig2: i32 = kani::any();
    kani::assume(sig1 >= 1 && sig1 <= 30);
    kani::assume(sig2 >= 1 && sig2 <= 30);
    let mut p = SigPending::new();
    p.add(sig1);
    if sig1 != sig2 { p.add(sig2); }
    assert_eq!(p.first_unmasked(0), p.first());
}

/// INV-SIG-K5: first_unmasked with exact mask returns None.
#[kani::proof]
fn verify_first_unmasked_exact_mask() {
    let sig: i32 = kani::any();
    kani::assume(sig >= 1 && sig <= 64);
    let mut p = SigPending::new();
    p.add(sig);
    let mask = 1u64 << (sig - 1);
    assert!(p.first_unmasked(mask).is_none());
}

/// INV-SIG-K6: has returns false for out-of-range signals.
#[kani::proof]
fn verify_has_out_of_range() {
    let mut p = SigPending::new();
    p.add(0);
    p.add(65);
    p.add(-1);
    assert!(!p.has(0));
    assert!(!p.has(65));
    assert!(!p.has(-1));
}

/// INV-SIG-K7: add is idempotent — only one bit set per signal.
#[kani::proof]
fn verify_add_idempotent() {
    let sig: i32 = kani::any();
    kani::assume(sig >= 1 && sig <= 64);
    let mut p = SigPending::new();
    p.add(sig);
    p.add(sig);
    p.add(sig);
    assert!(p.has(sig));
    assert_eq!(p.signal.count_ones(), 1);
}

/// INV-SIG-K8: SigAction::new().action() == Default.
#[kani::proof]
fn verify_sigaction_default() {
    let a = SigAction::new();
    assert_eq!(a.action(), SigActionKind::Default);
}

/// INV-SIG-K9: SigAction with non-sentinel handler has Handler action.
#[kani::proof]
fn verify_sigaction_handler() {
    let addr: usize = kani::any();
    kani::assume(addr > 2); // not Default(0) or Ignore(1)
    let a = SigAction {
        sa_handler: addr,
        sa_flags: 0,
        sa_mask: 0,
    };
    assert_eq!(a.action(), SigActionKind::Handler);
}
