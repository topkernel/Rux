//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! uaccess bounds-check property tests (framekernel Framework rail #1).
//!
//! `access_ok` is the pure-arithmetic gate every user pointer must pass
//! before the Framework's raw copy routines run; `clamp_rw_count` is the
//! syscall-side transfer-size clamp. Together they bound every uaccess
//! transfer from above (kernel space) and below (null page). These tests
//! pin the arithmetic so a refactor of either function cannot silently
//! admit kernel-space or wrapping ranges.
//!
//! Logic copied from:
//! - kernel/src/arch/riscv64/uaccess.rs (access_ok, MAX_RW_COUNT)
//! - kernel/src/arch/riscv64/mm/memory_layout.rs (USER_START/USER_END,
//!   TASK_SIZE, KERNEL_LINK_ADDR)
//! - kernel/src/syscall/io.rs (clamp_rw_count)
//! When the kernel versions change, these copies must be updated.

use proptest::prelude::*;

// ============================================================================
// Copied constants (kernel/src/arch/riscv64/mm/memory_layout.rs)
// ============================================================================

/// Sv39: PGDIR_SIZE = 1 GiB, PTRS_PER_PGD = 512, user half = 256 GiB.
pub const TASK_SIZE: usize = ((1u64 << 30) * 512 / 2) as usize;
pub const USER_START: usize = 0x0000_0000;
pub const USER_END: usize = TASK_SIZE;
pub const KERNEL_LINK_ADDR: usize = 0xffff_ffff_8000_0000;

// The framekernel isolation boundary, checked at compile time of this test:
// the entire user range accepted by access_ok lies strictly below the
// kernel's link address.
const _: () = assert!(USER_END < KERNEL_LINK_ADDR);

// ============================================================================
// Copied logic (kernel/src/arch/riscv64/uaccess.rs)
// ============================================================================

pub const MAX_RW_COUNT: usize = 0x7FFF_F000;

pub fn access_ok(addr: usize, size: usize) -> bool {
    if addr < USER_START {
        return false;
    }
    let end = match addr.checked_add(size) {
        Some(e) => e,
        None => return false,
    };
    end <= USER_END
}

// ============================================================================
// Copied logic (kernel/src/syscall/io.rs)
// ============================================================================

pub fn clamp_rw_count(count: usize) -> usize {
    if count > MAX_RW_COUNT { MAX_RW_COUNT } else { count }
}

// ============================================================================
// Strategies
// ============================================================================

/// usize mix biased to the boundaries that matter: 0, USER_END +/- small,
/// TASK_SIZE, MAX_RW_COUNT neighborhood, and usize::MAX wrap territory.
fn addr_strategy() -> impl Strategy<Value = usize> {
    prop_oneof![
        4 => any::<usize>(),
        1 => (USER_END as u32..=(USER_END as u32).saturating_add(16))
            .prop_map(|v| v as usize),
        1 => 0usize..64,
        1 => (USER_END - 64..=USER_END),
        1 => (usize::MAX - 64..=usize::MAX),
    ]
}

fn size_strategy() -> impl Strategy<Value = usize> {
    prop_oneof![
        4 => any::<usize>(),
        1 => 0usize..64,
        1 => (MAX_RW_COUNT - 16..=MAX_RW_COUNT + 16),
        1 => (usize::MAX - 64..=usize::MAX),
    ]
}

// ============================================================================
// Properties
// ============================================================================

/// Acceptance implies containment with no wrap: [addr, addr+size) lies in
/// [USER_START, USER_END]. This is the single invariant the Framework's raw
/// copy routines rely on.
proptest! {
    #[test]
    fn accepted_ranges_lie_in_user_space(addr in addr_strategy(), size in size_strategy()) {
        if access_ok(addr, size) {
            prop_assert!(addr >= USER_START);
            let end = addr.checked_add(size).expect("accepted range must not wrap");
            prop_assert!(end <= USER_END);
        }
    }
}

/// Anything at or above the kernel half boundary is rejected for every size
/// (the user-writable window can never reach kernel space).
proptest! {
    #[test]
    fn kernel_space_is_never_accepted(
        addr in (USER_END..=usize::MAX),
        size in size_strategy(),
    ) {
        prop_assert!(!access_ok(addr, size));
    }
}

/// Monotone in size: shrinking an accepted transfer keeps it accepted.
proptest! {
    #[test]
    fn acceptance_is_monotone_in_size(
        addr in addr_strategy(),
        size in size_strategy(),
        frac in 0usize..=100,
    ) {
        let shrink = size / 100 * frac;
        if access_ok(addr, size) {
            prop_assert!(access_ok(addr, size - shrink));
        }
    }
}

/// Interval join: a transfer is accepted iff it can be split into two
/// accepted halves (both directions), for non-wrapping base addresses.
proptest! {
    #[test]
    fn interval_split_join(
        addr in addr_strategy(),
        s1 in size_strategy(),
        s2 in size_strategy(),
    ) {
        prop_assume!(addr <= USER_END);
        let joined_ok = s1.checked_add(s2).map(|sum| access_ok(addr, sum)).unwrap_or(false);
        let split_ok = access_ok(addr, s1)
            && addr.checked_add(s1).map(|mid| access_ok(mid, s2)).unwrap_or(false);
        prop_assert_eq!(joined_ok, split_ok);
    }
}

/// Boundary pins.
#[test]
fn access_ok_boundaries() {
    assert!(access_ok(USER_START, 0));
    assert!(access_ok(USER_END, 0));
    assert!(!access_ok(USER_END, 1));
    assert!(!access_ok(USER_END + 1, 0));
    assert!(access_ok(USER_START, USER_END)); // full user window
    assert!(!access_ok(usize::MAX, 1)); // must not wrap
    assert!(!access_ok(usize::MAX, 0)); // above USER_END
    assert!(!access_ok(0, usize::MAX)); // end wraps past USER_END
}

/// Clamp invariants: bounded, idempotent, identity below the bound.
proptest! {
    #[test]
    fn clamp_is_bounded_idempotent_identity(count in any::<usize>()) {
        let c1 = clamp_rw_count(count);
        prop_assert!(c1 <= MAX_RW_COUNT);
        prop_assert_eq!(clamp_rw_count(c1), c1);
        if count <= MAX_RW_COUNT {
            prop_assert_eq!(c1, count);
        } else {
            prop_assert_eq!(c1, MAX_RW_COUNT);
        }
    }
}

/// Pair property: the clamped count can never push an in-range base address
/// out of the user window by itself (clamp bound 2 GiB is far below the
/// 256 GiB user window).
proptest! {
    #[test]
    fn clamped_count_never_causes_rejection_by_size(
        addr in 0usize..USER_END,
        count in any::<usize>(),
    ) {
        let clamped = clamp_rw_count(count);
        if addr + clamped > USER_END {
            // rejection, when it happens, is due to addr being near the top;
            // clamped itself is <= 2 GiB so addr must be within 2 GiB of the
            // top of user space
            prop_assert!(addr + MAX_RW_COUNT > USER_END);
        }
    }
}
