//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! x86_64 process/thread arch hooks.

/// Reset the FPU and TLS state on exec. X86-TODO(agent x86-trap):
/// zero the fxsave area, reset fsbase/gsbase to 0.
pub fn flush_thread() {}
