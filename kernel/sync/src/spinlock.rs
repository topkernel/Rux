//! The C-STYLE RAW SPINLOCK FREE FUNCTIONS (moved from the framework's
//! spinlock banner section — sync shrink lane 2026-11-27, sole copies
//! here): the `raw_spin_*` wrappers over the RawSpinlock methods, kept
//! for API-completeness with the Linux spin_lock()/spin_unlock() spell.
//!
//! Zero callers anywhere in the tree (compiler-verified at move time):
//! every in-tree consumer uses the guard methods or the Keyed* variants
//! directly. The RawSpinlock type and its impl stay framework-side (the
//! lock primitive interior — the pinned boundary; types.rs owns the
//! struct, the hotpath banner owns the impl).

use rux::sync::types::RawSpinlock;

/// spin_lock(): the C-style raw acquire.
#[inline(always)]
pub fn raw_spin_lock(l: &RawSpinlock) { l.lock(); }

/// spin_unlock(): the C-style raw release.
#[inline(always)]
pub fn raw_spin_unlock(l: &RawSpinlock) { l.unlock(); }

/// spin_is_locked(): the C-style raw state query.
#[inline(always)]
pub fn raw_spin_is_locked(l: &RawSpinlock) -> bool { l.is_locked() }

/// spin_trylock(): the C-style raw try-acquire.
#[inline(always)]
pub fn raw_spin_trylock(l: &mut RawSpinlock) -> bool { l.try_lock() }
