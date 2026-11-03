//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Sync subsystem verification tests.

pub mod spinlock_test;
pub mod seqlock_test;
pub mod futex_test;
pub mod rwlock_test;
pub mod semaphore_test;
pub mod mutex_test;

// S-1 model-checking pilot: faithful extraction of the SleepMutex
// handoff state machine (sync-checked against kernel/src/sync/mutex.rs)
// and its fast unit tests. The loom interleaving harness compiles only
// under `--cfg loom` + `--features loom` (see mutex_loom.rs header for
// the exact command); the kani proof harnesses compile only under
// `cargo kani` (wired in lib.rs).
pub mod mutex_model;
pub mod mutex_model_test;
#[cfg(all(feature = "loom", loom))]
pub mod mutex_loom;
