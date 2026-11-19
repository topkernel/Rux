//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Signal set, signal constants, and SigFlags invariant tests.
//!
//! Types copied from: kernel/src/signal.rs

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/signal.rs
// ============================================================================

pub type SigSet = u64;

pub const SIGRTMIN: i32 = 32;
pub const SIGRTMAX: i32 = 64;

pub mod sigprocmask_how {
    pub const SIG_BLOCK: i32 = 0;
    pub const SIG_UNBLOCK: i32 = 1;
    pub const SIG_SETMASK: i32 = 2;
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct SigFlags(pub u32);

impl SigFlags {
    pub const SA_NOCLDSTOP: u32 = 0x00000001;
    pub const SA_NOCLDWAIT: u32 = 0x00000002;
    pub const SA_SIGINFO: u32 = 0x00000004;
    pub const SA_ONSTACK: u32 = 0x08000000;
    pub const SA_RESTART: u32 = 0x10000000;
    pub const SA_NODEFER: u32 = 0x40000000;
    pub const SA_RESETHAND: u32 = 0x80000000;

    pub fn new(flags: u32) -> Self { Self(flags) }
    pub fn bits(&self) -> u32 { self.0 }
}

/// Standard signal values (1-22, matching the kernel enum)
#[repr(i32)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Signal {
    SIGHUP = 1, SIGINT = 2, SIGQUIT = 3, SIGILL = 4,
    SIGTRAP = 5, SIGABRT = 6, SIGBUS = 7, SIGFPE = 8,
    SIGKILL = 9, SIGUSR1 = 10, SIGSEGV = 11, SIGUSR2 = 12,
    SIGPIPE = 13, SIGALRM = 14, SIGTERM = 15, SIGSTKFLT = 16,
    SIGCHLD = 17, SIGCONT = 18, SIGSTOP = 19, SIGTSTP = 20,
    SIGTTIN = 21, SIGTTOU = 22,
}

// ============================================================================
// Helper functions for SigSet operations
// ============================================================================

fn sigset_add(set: &mut SigSet, sig: i32) {
    if sig >= 1 && sig <= 64 {
        *set |= 1u64 << ((sig - 1) as u32);
    }
}

fn sigset_has(set: SigSet, sig: i32) -> bool {
    if sig >= 1 && sig <= 64 {
        (set & (1u64 << ((sig - 1) as u32))) != 0
    } else {
        false
    }
}

fn sigset_remove(set: &mut SigSet, sig: i32) {
    if sig >= 1 && sig <= 64 {
        *set &= !(1u64 << ((sig - 1) as u32));
    }
}

fn sigset_clear(set: &mut SigSet) {
    *set = 0;
}

fn sigset_first(set: SigSet) -> Option<i32> {
    if set == 0 {
        return None;
    }
    let bit = set.trailing_zeros();
    Some((bit + 1) as i32)
}

fn sigset_first_unmasked(set: SigSet, mask: SigSet) -> Option<i32> {
    let unmasked = set & !mask;
    sigset_first(unmasked)
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-SIG-1: add then has returns true for valid signals
    #[test]
    fn test_add_has(sig in 1i32..65i32) {
        let mut set: SigSet = 0;
        sigset_add(&mut set, sig);
        prop_assert!(sigset_has(set, sig));
    }

    /// INV-SIG-2: add then remove then has returns false
    #[test]
    fn test_add_remove_has(sig in 1i32..65i32) {
        let mut set: SigSet = 0;
        sigset_add(&mut set, sig);
        sigset_remove(&mut set, sig);
        prop_assert!(!sigset_has(set, sig));
    }

    /// INV-SIG-3: clear makes set empty
    #[test]
    fn test_clear(s1 in 1i32..65i32, s2 in 1i32..65i32, s3 in 1i32..65i32) {
        let mut set: SigSet = 0;
        sigset_add(&mut set, s1);
        sigset_add(&mut set, s2);
        sigset_add(&mut set, s3);
        sigset_clear(&mut set);
        prop_assert_eq!(set, 0);
        prop_assert_eq!(sigset_first(set), None);
    }

    /// INV-SIG-4: first returns lowest pending signal
    #[test]
    fn test_first_lowest(higher in 5i32..65i32, lower in 1i32..5i32) {
        let mut set: SigSet = 0;
        sigset_add(&mut set, higher);
        sigset_add(&mut set, lower);
        prop_assert_eq!(sigset_first(set), Some(lower));
    }

    /// INV-SIG-5: first_unmasked with empty mask == first
    #[test]
    fn test_first_unmasked_no_mask(s1 in 1i32..65i32, s2 in 1i32..65i32) {
        let mut set: SigSet = 0;
        sigset_add(&mut set, s1);
        sigset_add(&mut set, s2);
        prop_assert_eq!(sigset_first_unmasked(set, 0), sigset_first(set));
    }

    /// INV-SIG-6: first_unmasked with full mask returns None
    #[test]
    fn test_first_unmasked_full_mask(s1 in 1i32..65i32, s2 in 1i32..65i32) {
        let mut set: SigSet = 0;
        sigset_add(&mut set, s1);
        sigset_add(&mut set, s2);
        prop_assert_eq!(sigset_first_unmasked(set, u64::MAX), None);
    }

    /// INV-SIG-7: has returns false for signals outside 1..=64
    #[test]
    fn test_has_out_of_range(sig in 65i32..100i32) {
        let mut set: SigSet = 0;
        sigset_add(&mut set, sig);
        prop_assert!(!sigset_has(set, sig));
    }

    /// INV-SIG-8: SigFlags round-trip through new/bits
    #[test]
    fn test_sigflags_roundtrip(flags in 0u32..u32::MAX) {
        let sf = SigFlags::new(flags);
        prop_assert_eq!(sf.bits(), flags);
    }

    /// INV-SIG-9: sigprocmask_how constants are 0, 1, 2
    #[test]
    fn test_sigprocmask_values(
        _v in 0u8..1u8,
    ) {
        prop_assert_eq!(sigprocmask_how::SIG_BLOCK, 0);
        prop_assert_eq!(sigprocmask_how::SIG_UNBLOCK, 1);
        prop_assert_eq!(sigprocmask_how::SIG_SETMASK, 2);
    }
}

#[test]
/// INV-SIG-10: SIGRTMIN and SIGRTMAX define valid RT signal range
fn test_rt_signal_range() {
    assert!(SIGRTMIN > 22); // RT signals start after standard signals
    assert!(SIGRTMAX > SIGRTMIN);
    assert_eq!(SIGRTMIN, 32);
    assert_eq!(SIGRTMAX, 64);
}

#[test]
/// INV-SIG-11: Signal enum discriminants match their signal numbers
fn test_signal_discriminants() {
    assert_eq!(Signal::SIGHUP as i32, 1);
    assert_eq!(Signal::SIGINT as i32, 2);
    assert_eq!(Signal::SIGQUIT as i32, 3);
    assert_eq!(Signal::SIGILL as i32, 4);
    assert_eq!(Signal::SIGTRAP as i32, 5);
    assert_eq!(Signal::SIGABRT as i32, 6);
    assert_eq!(Signal::SIGBUS as i32, 7);
    assert_eq!(Signal::SIGFPE as i32, 8);
    assert_eq!(Signal::SIGKILL as i32, 9);
    assert_eq!(Signal::SIGUSR1 as i32, 10);
    assert_eq!(Signal::SIGSEGV as i32, 11);
    assert_eq!(Signal::SIGUSR2 as i32, 12);
    assert_eq!(Signal::SIGPIPE as i32, 13);
    assert_eq!(Signal::SIGALRM as i32, 14);
    assert_eq!(Signal::SIGTERM as i32, 15);
    assert_eq!(Signal::SIGSTKFLT as i32, 16);
    assert_eq!(Signal::SIGCHLD as i32, 17);
    assert_eq!(Signal::SIGCONT as i32, 18);
    assert_eq!(Signal::SIGSTOP as i32, 19);
    assert_eq!(Signal::SIGTSTP as i32, 20);
    assert_eq!(Signal::SIGTTIN as i32, 21);
    assert_eq!(Signal::SIGTTOU as i32, 22);
}

#[test]
/// INV-SIG-12: SIGKILL and SIGSTOP are 9 and 19 respectively
fn test_uncatchable_signals() {
    assert_eq!(Signal::SIGKILL as i32, 9);
    assert_eq!(Signal::SIGSTOP as i32, 19);
}

#[test]
/// INV-SIG-13: SigFlags values are distinct powers of two
fn test_sigflags_powers_of_two() {
    let flags = [
        ("SA_NOCLDSTOP", SigFlags::SA_NOCLDSTOP),
        ("SA_NOCLDWAIT", SigFlags::SA_NOCLDWAIT),
        ("SA_SIGINFO", SigFlags::SA_SIGINFO),
        ("SA_ONSTACK", SigFlags::SA_ONSTACK),
        ("SA_RESTART", SigFlags::SA_RESTART),
        ("SA_NODEFER", SigFlags::SA_NODEFER),
        ("SA_RESETHAND", SigFlags::SA_RESETHAND),
    ];
    let mut seen = std::collections::HashSet::new();
    for (name, val) in &flags {
        assert!(*val > 0 && (*val & (*val - 1)) == 0,
            "{} ({:#x}) is not a power of two", name, val);
        assert!(seen.insert(*val), "{} is a duplicate flag", name);
    }
}

#[test]
/// INV-SIG-14: SigSet can represent all 64 signals
fn test_sigset_capacity() {
    let mut set: SigSet = 0;
    for sig in 1..=64 {
        sigset_add(&mut set, sig);
    }
    for sig in 1..=64 {
        assert!(sigset_has(set, sig), "signal {} should be in set", sig);
    }
    assert_eq!(set, u64::MAX);
}
