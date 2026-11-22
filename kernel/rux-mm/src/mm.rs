//! The mm tree, nested under crate::mm (the rux-fs recipe): the §4
//! policy files flipped into this crate (M1b) live here as physical
//! sole-copy files, and EVERY framework mm item is glob-re-exported —
//! so `super::X` / `crate::mm::X` inside the moved files resolve
//! exactly as they did framework-side, and the explicit module
//! declarations below shadow the glob for the flipped names (the
//! framework's mm/mod.rs drops each declaration as its file flips).
//!
//! Flipped so far (M1b): hugepage, kswapd, oom_kill, vmscan, lru, compact.

pub mod hugepage;
pub mod kswapd;
pub mod oom_kill;
pub mod vmscan;
pub mod lru;
pub mod compact;

pub use rux::mm::*;
