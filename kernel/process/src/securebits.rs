//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! prctl securebits values that MOVE (p23-shrink-process). The pinned
//! impl Cred keeps SECBIT_NO_SETUID_FIXUP(+_LOCKED) core-side (read
//! in-file by keeps_caps_on_setuid); this file holds the values whose
//! only consumers are the syscall layer.

#![forbid(unsafe_code)]
/// SECBIT_KEEP_CAPS (bit 4) in the prctl securebits encoding.
pub const SECBIT_KEEP_CAPS: u32 = 1 << 4;
