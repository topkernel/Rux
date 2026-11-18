//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 uaccess bounds-check property tests.
//!
//! `access_ok` is the pure-arithmetic gate every user pointer must pass
//! before the raw copy routines run. The x86_64 twin has NO low floor
//! check (unlike riscv64): the only invariant is the checked-add ceiling
//! at USER_END, so a null/near-zero address is accepted by this gate and
//! must be rejected by callers that need it (e.g. strncpy's is_null).
//!
//! Logic copied from:
//! - kernel/src/arch/x86_64/uaccess.rs (access_ok)
//! - kernel/src/arch/x86_64/mm/memory_layout.rs (TASK_SIZE / USER_END)
//! When the kernel versions change, these copies must be updated.

use proptest::prelude::*;

// ============================================================================
// Copied constants (kernel/src/arch/x86_64/mm/memory_layout.rs)
// ============================================================================

/// x86_64 user space: 0 .. 0x0000_7fff_ffff_f000 (128 TiB).
pub const TASK_SIZE: usize = 0x0000_7fff_ffff_f000;
pub const USER_START: usize = 0x0000_0000;
pub const USER_END: usize = TASK_SIZE;

// ============================================================================
// Copied logic (kernel/src/arch/x86_64/uaccess.rs)
// ============================================================================

pub fn access_ok(addr: usize, size: usize) -> bool {
    addr.checked_add(size).map_or(false, |end| end <= USER_END)
}

// ============================================================================
// Properties
// ============================================================================

/// Acceptance implies containment with no wrap.
proptest! {
    #[test]
    fn accepted_ranges_lie_in_user_space(addr in any::<usize>(), size in any::<usize>()) {
        if access_ok(addr, size) {
            let end = addr.checked_add(size).expect("accepted range must not wrap");
            prop_assert!(end <= USER_END);
        }
    }
}

/// Anything whose end crosses the user ceiling is rejected.
proptest! {
    #[test]
    fn kernel_space_is_never_accepted(addr in (USER_END..=usize::MAX), size in any::<usize>()) {
        prop_assert!(!access_ok(addr, size));
    }
}

/// Monotone in size: shrinking an accepted transfer keeps it accepted.
proptest! {
    #[test]
    fn acceptance_is_monotone_in_size(addr in any::<usize>(), size in any::<usize>(), frac in 0usize..=100) {
        let shrink = size / 100 * frac;
        if access_ok(addr, size) {
            prop_assert!(access_ok(addr, size - shrink));
        }
    }
}

/// Interval join: accepted iff both halves are accepted. The base is
/// drawn directly from the user window: a pure any::<usize>() base is
/// above USER_END with overwhelming probability and would reject its way
/// into a proptest abort.
proptest! {
    #[test]
    fn interval_split_join(addr in 0usize..=USER_END, s1 in any::<usize>(), s2 in any::<usize>()) {
        let joined_ok = s1.checked_add(s2).map(|sum| access_ok(addr, sum)).unwrap_or(false);
        let split_ok = access_ok(addr, s1)
            && addr.checked_add(s1).map(|mid| access_ok(mid, s2)).unwrap_or(false);
        prop_assert_eq!(joined_ok, split_ok);
    }
}

/// Boundary pins. Note the deliberate x86_64 semantics: no floor check,
/// so address 0 IS accepted by this gate (callers own the null check).
#[test]
fn access_ok_boundaries() {
    assert!(access_ok(0, 1)); // null accepted here by design
    assert!(access_ok(USER_END, 0));
    assert!(!access_ok(USER_END, 1));
    assert!(!access_ok(usize::MAX, 0));
    assert!(!access_ok(USER_END - 1, usize::MAX)); // wrap
}
