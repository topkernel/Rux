//! The RWLOCK PREEMPT BOOK-KEEPING FREE FUNCTION (moved from the
//! framework's rwlock banner inline helpers — sync shrink lane
//! 2026-11-27, sole copy here): the plain `rwlock_preempt_disable()`.
//!
//! Zero callers in the tree (compiler-verified at move time): the
//! guard implementations and every lock helper use the
//! `#[track_caller]` `rwlock_preempt_disable_at(loc)` variant instead,
//! which stays framework-side with the rwlock primitive interior (the
//! pinned boundary). The moved plain wrapper books the caller site the
//! same way; it reaches the published-core preempt counter helpers
//! cross-crate (rux::interrupt::preempt, pub).

/// rwlock-flavored preempt book-keeping without an explicit location:
/// the #![track_caller] Site reports the direct caller (parity with the
/// framework's rwlock_preempt_disable_at).
#[inline]
#[track_caller]
pub fn rwlock_preempt_disable() {
    rux::interrupt::preempt::preempt_count_add_at(
        rux::interrupt::preempt::PREEMPT_OFFSET,
        core::panic::Location::caller(),
    );
}
