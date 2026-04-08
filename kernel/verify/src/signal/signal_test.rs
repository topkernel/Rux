//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Signal bitmap and SigAction invariant tests.
//!
//! Types copied from: kernel/src/signal.rs (AtomicU64 → plain u64 for std testing)

use proptest::prelude::*;

// ============================================================================
// Copied types from kernel/src/signal.rs
// ============================================================================

pub const SIGRTMIN: i32 = 32;
pub const SIGRTMAX: i32 = 64;

#[repr(i32)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Signal {
    SIGHUP = 1,
    SIGINT = 2,
    SIGQUIT = 3,
    SIGILL = 4,
    SIGTRAP = 5,
    SIGABRT = 6,
    SIGBUS = 7,
    SIGFPE = 8,
    SIGKILL = 9,
    SIGUSR1 = 10,
    SIGSEGV = 11,
    SIGUSR2 = 12,
    SIGPIPE = 13,
    SIGALRM = 14,
    SIGTERM = 15,
    SIGSTKFLT = 16,
    SIGCHLD = 17,
    SIGCONT = 18,
    SIGSTOP = 19,
    SIGTSTP = 20,
    SIGTTIN = 21,
    SIGTTOU = 22,
}

/// Verify-local SigPending using plain u64 instead of AtomicU64.
pub struct SigPending {
    pub signal: u64,
}

impl SigPending {
    pub fn new() -> Self {
        Self { signal: 0 }
    }

    pub fn add(&mut self, sig: i32) {
        if sig < 1 || sig > 64 {
            return;
        }
        let mask = 1u64 << (sig - 1);
        self.signal |= mask;
    }

    pub fn remove(&mut self, sig: i32) {
        if sig < 1 || sig > 64 {
            return;
        }
        let mask = 1u64 << (sig - 1);
        self.signal &= !mask;
    }

    pub fn has(&self, sig: i32) -> bool {
        if sig < 1 || sig > 64 {
            return false;
        }
        let mask = 1u64 << (sig - 1);
        (self.signal & mask) != 0
    }

    pub fn first(&self) -> Option<i32> {
        if self.signal == 0 {
            return None;
        }
        let sig = self.signal.trailing_zeros() as i32 + 1;
        Some(sig)
    }

    pub fn first_unmasked(&self, mask: u64) -> Option<i32> {
        let deliverable = self.signal & !mask;
        if deliverable == 0 {
            return None;
        }
        let sig = deliverable.trailing_zeros() as i32 + 1;
        Some(sig)
    }

    pub fn get_all(&self) -> u64 {
        self.signal
    }

    pub fn clear(&mut self) {
        self.signal = 0;
    }
}

/// Verify-local SigAction using sentinel addresses instead of function pointers.
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum SigActionKind {
    Default = 0,
    Ignore = 1,
    Handler = 2,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq)]
pub struct SigAction {
    pub sa_handler: usize,
    pub sa_flags: u32,
    pub sa_mask: u64,
}

impl SigAction {
    fn default_handler() -> usize {
        SigActionKind::Default as usize
    }

    fn ignore_handler() -> usize {
        SigActionKind::Ignore as usize
    }

    pub fn new() -> Self {
        Self {
            sa_handler: Self::default_handler() as usize,
            sa_flags: 0,
            sa_mask: 0,
        }
    }

    pub fn ignore() -> Self {
        Self {
            sa_handler: Self::ignore_handler() as usize,
            sa_flags: 0,
            sa_mask: 0,
        }
    }

    pub fn handler(handler: usize, flags: u32) -> Self {
        Self {
            sa_handler: handler,
            sa_flags: flags,
            sa_mask: 0,
        }
    }

    pub fn action(&self) -> SigActionKind {
        if self.sa_handler == Self::default_handler() as usize {
            SigActionKind::Default
        } else if self.sa_handler == Self::ignore_handler() as usize {
            SigActionKind::Ignore
        } else {
            SigActionKind::Handler
        }
    }

    pub fn has_handler(&self) -> bool {
        self.action() == SigActionKind::Handler
    }
}

/// Verify-local signal mask operations (plain u64 instead of AtomicU64 + RwSpinlock).
pub struct SignalMask {
    pub mask: u64,
}

impl SignalMask {
    pub fn new() -> Self {
        Self { mask: 0 }
    }

    pub fn add_mask(&mut self, sig: i32) {
        if sig < 1 || sig > 64 {
            return;
        }
        let m = 1u64 << (sig - 1);
        self.mask |= m;
    }

    pub fn remove_mask(&mut self, sig: i32) {
        if sig < 1 || sig > 64 {
            return;
        }
        let m = 1u64 << (sig - 1);
        self.mask &= !m;
    }

    pub fn is_masked(&self, sig: i32) -> bool {
        if sig < 1 || sig > 64 {
            return false;
        }
        let m = 1u64 << (sig - 1);
        (self.mask & m) != 0
    }
}

// ============================================================================
// Tests
// ============================================================================

proptest! {
    /// INV-SIG-1: add(sig); has(sig) for sig in 1..=64
    #[test]
    fn test_add_has(sig in 1i32..65i32) {
        let mut p = SigPending::new();
        p.add(sig);
        prop_assert!(p.has(sig));
    }

    /// INV-SIG-2: add(sig); remove(sig); !has(sig) roundtrip
    #[test]
    fn test_add_remove_roundtrip(sig in 1i32..65i32) {
        let mut p = SigPending::new();
        p.add(sig);
        prop_assert!(p.has(sig));
        p.remove(sig);
        prop_assert!(!p.has(sig));
    }

    /// INV-SIG-3: first() returns lowest set bit
    #[test]
    fn test_first(sig1 in 1i32..65i32, sig2 in 1i32..65i32) {
        let mut p = SigPending::new();
        p.add(sig1);
        p.add(sig2);
        let first = p.first().unwrap();
        prop_assert_eq!(first, std::cmp::min(sig1, sig2));
    }

    /// INV-SIG-4: first_unmasked(0) == first()
    #[test]
    fn test_first_unmasked_no_mask(
        sig1 in 1i32..30i32,
        sig2 in 1i32..30i32,
    ) {
        let mut p = SigPending::new();
        p.add(sig1);
        if sig1 != sig2 {
            p.add(sig2);
        }
        prop_assert_eq!(p.first_unmasked(0), p.first());
    }

    /// INV-SIG-5: first_unmasked returns None when all masked
    #[test]
    fn test_first_unmasked_all_masked(sig in 1i32..65i32) {
        let mut p = SigPending::new();
        p.add(sig);
        let mask = 1u64 << (sig - 1);
        prop_assert!(p.first_unmasked(mask).is_none());
    }

    /// INV-SIG-6: has(0) and has(65) return false (out of range)
    #[test]
    fn test_has_out_of_range(_v in 0u8..1u8) {
        let mut p = SigPending::new();
        p.add(0);
        p.add(65);
        p.add(-1);
        prop_assert!(!p.has(0));
        prop_assert!(!p.has(65));
        prop_assert!(!p.has(-1));
    }

    /// INV-SIG-7: get_all() bitmap matches added signals
    #[test]
    fn test_get_all(
        sigs in proptest::collection::vec(1i32..65i32, 1..10),
    ) {
        let mut p = SigPending::new();
        let mut expected: u64 = 0;
        for &sig in &sigs {
            p.add(sig);
            expected |= 1u64 << (sig - 1);
        }
        prop_assert_eq!(p.get_all(), expected);
    }

    /// INV-SIG-8: SigAction::new().action() == Default
    #[test]
    fn test_sigaction_default(_v in 0u8..1u8) {
        let a = SigAction::new();
        prop_assert_eq!(a.action(), SigActionKind::Default);
        prop_assert!(!a.has_handler());
    }

    /// INV-SIG-9: SigAction::ignore().action() == Ignore
    #[test]
    fn test_sigaction_ignore(_v in 0u8..1u8) {
        let a = SigAction::ignore();
        prop_assert_eq!(a.action(), SigActionKind::Ignore);
        prop_assert!(!a.has_handler());
    }

    /// INV-SIG-10: SigAction::handler() has handler
    #[test]
    fn test_sigaction_handler(addr in 0x1000usize..0x10000usize) {
        let a = SigAction::handler(addr, 0);
        prop_assert_eq!(a.action(), SigActionKind::Handler);
        prop_assert!(a.has_handler());
    }

    /// INV-SIG-11: SignalMask add/remove/is_masked
    #[test]
    fn test_signal_mask(sig in 1i32..65i32) {
        let mut m = SignalMask::new();
        prop_assert!(!m.is_masked(sig));
        m.add_mask(sig);
        prop_assert!(m.is_masked(sig));
        m.remove_mask(sig);
        prop_assert!(!m.is_masked(sig));
    }

    /// INV-SIG-12: SignalMask out of range ignored
    #[test]
    fn test_signal_mask_out_of_range(_v in 0u8..1u8) {
        let mut m = SignalMask::new();
        m.add_mask(0);
        m.add_mask(65);
        m.add_mask(-1);
        prop_assert_eq!(m.mask, 0);
    }

    /// INV-SIG-13: Randomized add/remove/first_unmasked sequence
    #[test]
    fn test_random_sequence(
        ops in proptest::collection::vec(
            (1i32..65i32, proptest::bool::ANY),
            0..20
        ),
    ) {
        let mut p = SigPending::new();
        let mut mask = SignalMask::new();
        for (sig, do_add) in ops {
            if do_add {
                p.add(sig);
                mask.add_mask(sig);
            } else {
                p.remove(sig);
                mask.remove_mask(sig);
            }
        }
        // Verify: first_unmasked should match manual check
        let expected = p.first_unmasked(mask.mask);
        let manual: Option<i32> = (1..=64)
            .find(|&s| p.has(s) && !mask.is_masked(s));
        prop_assert_eq!(expected, manual);
    }

    /// INV-SIG-14: add is idempotent
    #[test]
    fn test_add_idempotent(sig in 1i32..65i32) {
        let mut p = SigPending::new();
        p.add(sig);
        p.add(sig);
        p.add(sig);
        prop_assert!(p.has(sig));
        // Only one bit should be set
        prop_assert_eq!(p.get_all().count_ones(), 1);
    }

    /// INV-SIG-15: clear removes everything
    #[test]
    fn test_clear(
        sigs in proptest::collection::vec(1i32..65i32, 1..10),
    ) {
        let mut p = SigPending::new();
        for &sig in &sigs {
            p.add(sig);
        }
        p.clear();
        prop_assert!(p.get_all() == 0);
        prop_assert!(p.first().is_none());
    }

    /// INV-SIG-16: first() returns None on empty
    #[test]
    fn test_first_empty(_v in 0u8..1u8) {
        let p = SigPending::new();
        prop_assert!(p.first().is_none());
    }
}
