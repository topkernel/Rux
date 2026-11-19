//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The unsafe-fn-call wrapper discipline (unsafe-slim round 3 lane A).
//!
//! The round-1/2 histogram put 15% of the unsafe mass on
//! `unsafe { some_unsafe_fn(args) }` at call sites — the SAME
//! precondition restated (or trusted silently) at every caller. The
//! discipline here flips it: the precondition is discharged ONCE, at
//! a typed boundary, and every caller is safe code.
//!
//! # The three rules
//!
//! * **INV-SAFECALL-1 (one wrapper)** — an `unsafe fn` whose
//!   precondition can be discharged once (not per call site) gets
//!   exactly ONE safe entry point in the defining module. The
//!   wrapper's doc names the invariant it discharges (`INV-*`); its
//!   body contains the single audited unsafe call.
//! * **INV-SAFECALL-2 (typed boundary)** — the wrapper's signature
//!   must make violations either impossible or loud: take `&T` /
//!   typed handles instead of raw pointers, bounds-check indices,
//!   panic (not UB) on structurally impossible inputs.
//! * **INV-SAFECALL-3 (no self-recursion)** — the round-2 bug class:
//!   never route the wrapper's own call sites (or the unsafe
//!   original's internals) back through the wrapper or the unsafe
//!   original — the original becomes private/deprecated, so the only
//!   unsafe call in the tree is inside the wrapper.
//!
//! # `safe_call!`
//!
//! [`safe_call!`] generates the wrapper mechanically: the unsafe call
//! exists exactly once (inside the macro expansion), the wrapper
//! carries the doc, and call sites are safe. For wrappers that need
//! argument checks, write them by hand following the same three
//! rules.
//!
//! # `safety_cast`
//!
//! [`safety_cast`] is the same discipline for transmute-shaped edges:
//! a size/repr compile-time assertion plus ONE audited
//! `core::mem::transmute` (INV-SAFECAST-1: same-size, both `Copy`,
//! assertion in a `const` item that breaks the build if either type
//! drifts).

/// Generate a safe wrapper around one `unsafe fn` (INV-SAFECALL-1/2/3).
///
/// ```ignore
/// safe_call! {
///     /// Node data for mutation during reclaim.
///     /// INV: serialized by the zone reclaim path (see vmscan).
///     pub fn node_for_reclaim() -> Option<&'static mut PglistData>
///         = unsafe pglist::first_online_node_mut;
/// }
/// ```
///
/// The generated function is `#[inline(always)]`; the doc (including
/// the INV line) lands on the wrapper; the unsafe call exists only in
/// the expansion.
#[macro_export]
macro_rules! safe_call {
    ($(#[$m:meta])* $vis:vis fn $name:ident($($arg:ident: $ty:ty),* $(,)?) -> $ret:ty = unsafe $callee:expr $(;)?) => {
        $(#[$m])*
        #[inline(always)]
        $vis fn $name($($arg: $ty),*) -> $ret {
            // SAFETY: INV-SAFECALL-1 — this wrapper's doc names the
            // discharged invariant; the types carry the proof
            // (INV-SAFECALL-2).
            $callee($($arg),*)
        }
    };
    ($(#[$m:meta])* $vis:vis fn $name:ident($($arg:ident: $ty:ty),* $(,)?) = unsafe $callee:expr $(;)?) => {
        $crate::safe_call! {
            $(#[$m])*
            $vis fn $name($($arg: $ty),*) -> () = unsafe $callee;
        }
    };
}

/// Compile-time same-size/repr assertion for cast edges
/// (INV-SAFECAST-1). Emit as a `const _: () = ...;` in the module
/// that owns the cast.
#[macro_export]
macro_rules! assert_layout_eq {
    ($a:ty, $b:ty) => {
        const _: () = {
            core::assert!(core::mem::size_of::<$a>() == core::mem::size_of::<$b>());
        };
    };
}

/// The audited `transmute` edge: reinterprets `src` as `U`
/// (INV-SAFECAST-1: caller asserts sizes match — checked at runtime
/// because `U`'s size is not usable in `const` contexts for generic
/// code on all toolchains; the assert makes drift LOUD instead of
/// silent truncation/padding reads).
#[inline(always)]
pub fn safety_cast<Src: Copy, Dst: Copy>(src: Src) -> Dst {
    assert!(
        core::mem::size_of::<Src>() == core::mem::size_of::<Dst>(),
        "safety_cast size drift: {} vs {}",
        core::mem::size_of::<Src>(),
        core::mem::size_of::<Dst>(),
    );
    // SAFETY: INV-SAFECAST-1 — sizes asserted equal above; both types
    // are Copy (no drop glue to skip); this is the ONE audited cast
    // edge for kernel layout casts (transmute_copy copies exactly
    // size_of::<Dst>() bytes, which the assert proves == Src's).
    unsafe { core::mem::transmute_copy(&src) }
}
