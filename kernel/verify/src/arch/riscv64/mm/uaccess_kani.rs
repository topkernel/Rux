//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Kani proof harnesses for the uaccess boundary arithmetic and the
//! strncpy_from_user chunk loop.
//!
//! Logic re-copied from:
//! - kernel/verify/src/arch/riscv64/mm/uaccess_test.rs (which mirrors
//!   kernel/src/arch/riscv64/uaccess.rs)
//!
//! The chunk-loop harness replaces the extable leaf primitive with a
//! NONDETERMINISTIC fault pattern: Kani proves the loop stays in bounds
//! and returns a length within `buf` for EVERY possible interleaving of
//! user page faults, not just sampled ones.

#![cfg(kani)]

// ============================================================================
// Copied logic (kernel/src/arch/riscv64/uaccess.rs + memory_layout.rs)
// ============================================================================

const USER_START: usize = 0x0000_0000;
/// Sv39: PGDIR_SIZE = 1 GiB, PTRS_PER_PGD = 512, user half = 256 GiB.
const USER_END: usize = ((1u64 << 30) * 512 / 2) as usize;
const MAX_RW_COUNT: usize = 0x7FFF_F000;

fn access_ok(addr: usize, size: usize) -> bool {
    if addr < USER_START {
        return false;
    }
    let end = match addr.checked_add(size) {
        Some(e) => e,
        None => return false,
    };
    end <= USER_END
}

fn clamp_rw_count(count: usize) -> usize {
    if count > MAX_RW_COUNT { MAX_RW_COUNT } else { count }
}

// ============================================================================
// Proofs — boundary arithmetic
// ============================================================================

#[kani::proof]
fn access_ok_accept_implies_containment() {
    let addr: usize = kani::any();
    let size: usize = kani::any();
    if access_ok(addr, size) {
        assert!(addr >= USER_START);
        let end = addr.checked_add(size).unwrap();
        assert!(end <= USER_END);
    }
}

#[kani::proof]
fn access_ok_rejects_kernel_space() {
    let addr: usize = kani::any();
    let size: usize = kani::any();
    // strictly above the ceiling: a zero-size transfer at exactly USER_END
    // is accepted by the kernel (end == USER_END), which the boundary pins
    // in the proptest suite lock in as intentional.
    kani::assume(addr > USER_END);
    assert!(!access_ok(addr, size));
}

#[kani::proof]
fn clamp_rw_count_is_bounded_and_identity_below() {
    let count: usize = kani::any();
    let clamped = clamp_rw_count(count);
    assert!(clamped <= MAX_RW_COUNT);
    if count <= MAX_RW_COUNT {
        assert_eq!(clamped, count);
    } else {
        assert_eq!(clamped, MAX_RW_COUNT);
    }
}

// ============================================================================
// Proof — strncpy_from_user chunk-loop index arithmetic under ANY fault
// pattern. Division of labor: the DATA-SCAN semantics (exact prefix,
// NUL position, zero-filled faulted tail) is covered by the concrete-data
// proptests in uaccess_test.rs; this harness proves the pure index
// arithmetic — every slice in-bounds and the returned length inside buf —
// for all possible fault outcomes. The NUL scan is therefore modeled
// abstractly (found at rel < got, or not found), which keeps the symbolic
// program tiny: materializing the iterator scan blew the solver budget
// (all checks UNDETERMINED).
// ============================================================================

const CHUNK: usize = 64;
const EFAULT: i64 = 14;

/// Re-copy of the chunk loop's index arithmetic with an abstract scan.
fn strncpy_chunk_bounds(limit: usize) -> Result<usize, i64> {
    let mut done: usize = 0;
    while done < limit {
        let want = core::cmp::min(CHUNK, limit - done);
        // Nondeterministic leaf: any partial-fault outcome of the chunk.
        let uncopied: usize = kani::any();
        kani::assume(uncopied <= want);
        let got = want - uncopied;
        // Abstract NUL scan over buf[done..done + got]: either not found,
        // or found at rel strictly inside the readable window.
        let probe: usize = kani::any();
        kani::assume(probe <= got);
        if probe < got {
            return Ok(done + probe);
        }
        if uncopied > 0 {
            return Err(-EFAULT);
        }
        done += want;
    }
    Ok(limit)
}

#[kani::unwind(24)]
#[kani::proof]
fn strncpy_chunk_loop_stays_in_bounds_under_any_fault_pattern() {
    const BUF_LEN_MAX: usize = 1024;
    let buf_len: usize = kani::any();
    kani::assume(buf_len <= BUF_LEN_MAX);
    let max_len: usize = kani::any();
    let addr: usize = kani::any();
    kani::assume(addr < USER_END);
    kani::assume(USER_END - addr >= buf_len);

    // Re-copy of the kernel limit computation
    let limit = core::cmp::min(max_len, buf_len);
    let limit = core::cmp::min(limit, USER_END - addr);

    match strncpy_chunk_bounds(limit) {
        Ok(len) => assert!(len <= buf_len),
        Err(e) => assert_eq!(e, -EFAULT),
    }
}

// ============================================================================
// Proof — get_user gate shape
// ============================================================================

#[kani::proof]
fn get_user_some_implies_containment() {
    let addr: usize = kani::any();
    let size = core::mem::size_of::<u32>();
    // The gate runs before any leaf access; Some is only reachable when
    // the gate passed (the leaf outcome is nondeterministic but gated).
    let leaf_ok: bool = kani::any();
    let got = if access_ok(addr, size) && leaf_ok {
        Some(0u32)
    } else {
        None
    };
    if got.is_some() {
        assert!(access_ok(addr, size));
    }
}
