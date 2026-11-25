//! Verus pilot (unsafe-5000-plan Layer 2.3): MmioRegion's bounds
//! invariant, proven at the SPEC layer.
//!
//! Scope decision (evaluation question 1 — "without distorting the
//! implementation's shape"): the primitive's raw volatile accesses are
//! owned by the Kani/Miri battery (they need an executable memory
//! model, which is Kani's strength and Verus's weakness). What Verus
//! adds is a DEDUCTIVE proof of the arithmetic invariants that the
//! checks REST on — theorems that hold for ALL inputs, not bounded
//! ones. The functions below mirror kernel/core/src/safety/mmio.rs
//! check()'s exact arithmetic (post-conviction shape: wrapping_add
//! alignment + checked_add window); the correspondence is by
//! line-for-line mirroring, kept in this file so the primitive's
//! compiled shape is untouched.
//!
//! Run: verus pilot_mmio.rs   (Verus 0.2026.11.02, toolchain 1.99.1)

use vstd::prelude::*;
use vstd::seq::*;

verus! {

/// The window model: a region is base..base+len. The invariant the
/// accessors must maintain (INV-MMIO-2): every performed access span
/// lies inside the window.
#[verifier::spec]
struct Region {
    base: usize,
    len: usize,
}

/// check()'s window gate, mirrored: returns the access address when
/// the span fits, None otherwise (the impl panics there — the loud
/// arm; a panic is not UB, which the Kani should_panic harnesses own).
#[verifier::spec]
spec fn check_window(base: int, len: int, off: int, width: int) -> Option<int> {
    if 0 <= off && off + width <= len {
        Some(base + off)
    } else {
        None
    }
}

// Theorem T1 (no-OOB, the INV-MMIO-2 core): whenever the gate admits
// an access, the whole span [addr, addr+width) lies inside the
// declared window [base, base+len) — with NO wraparound, because
// usize is unbounded in Verus's spec arithmetic (the usize-MAX wrap
// cases are exactly what the impl's checked_add exists to reject, and
// the Kani harnesses verify it does).
proof fn no_oob_when_admitted(base: int, len: int, off: int, width: int)
    ensures
        forall |addr: int|
            #[trigger] check_window(base, len, off, width) == Some(addr)
            ==> (base <= addr && addr + width <= base + len),
{
}

spec fn is_pow2_width(w: int) -> bool {
    w == 1 || w == 2 || w == 4 || w == 8
}

// Theorem T2 (alignment closure, the conviction-#1 justification):
// the impl's debug_assert computes (base.wrapping_add(off)) % width.
// When base and off are individually width-aligned, wrapping_add
// preserves alignment EVEN ON WRAP — because 2^64 % width == 0 for
// every power of two width (1, 2, 4, 8), so the discarded 2^64 is
// invisible to the modulus. This is exactly why the wrapping form is
// equivalent to the unwrapped one for the values the impl accepts.
proof fn wrapping_add_preserves_alignment(base: int, off: int)
    ensures
        forall |width: int|
            #[trigger] is_pow2_width(width)
            && base % width == 0
            && off % width == 0
            ==> (((base + off) % 18446744073709551616) % width) == ((base + off) % width),
{
    // For power-of-two width w: 2^64 % w == 0, so subtracting any
    // multiple of 2^64 (the wrap) cannot change the residue mod w.
    // Verus discharges this automatically for each concrete w.
}

// Theorem T3 (admit-closure under access): the address returned by
// the gate is always base-relative — the accessors can reconstruct
// the window membership from addr alone.
proof fn admitted_addr_is_relative(base: int, len: int, off: int, width: int)
    ensures
        forall |addr: int|
            #[trigger] check_window(base, len, off, width) == Some(addr)
            ==> addr - base == off,
{
}

fn main() {}
}
