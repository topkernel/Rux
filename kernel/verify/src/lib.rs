//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Property-based tests for Rux kernel core data structure invariants.
//!
//! These tests copy pure algorithmic logic from kernel/src/ and verify
//! safety invariants using proptest (randomized input generation).
//!
//! The types and functions tested here are copied directly from the kernel
//! source to avoid a shared-crate dependency chain. When kernel types change,
//! the copies here must be updated accordingly.
//!
//! Run: `cargo test -p rux-verify`

pub mod mm;
pub mod sync;
pub mod arch;
