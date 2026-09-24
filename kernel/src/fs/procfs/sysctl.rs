//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/sys minimal tree (P1) — global sysctl variables with read/write
//! procfs files.
//!
//! Implemented nodes:
//!   /proc/sys/kernel/hostname       rw (Linux: uts; <= 64 bytes, no NUL)
//!   /proc/sys/kernel/pid_max        rw (default 32768; clamped 2..=4194304)
//!   /proc/sys/kernel/ostype         r  ("Linux" — compat for userland)
//!   /proc/sys/vm/overcommit_memory  rw (default 0; stored, NO effect on
//!                                    the allocator — recorded divergence)
//!   /proc/sys/fs/file-max           rw (default 4096*16 per our MAX_FDS;
//!                                    value stored only — no enforcement)
//!
//! Semantic boundary: these are global kernel variables, not per-mount
//! sysctls; netns/utsns isolation does not exist (namespaces are a
//! separate P1).

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use crate::sync::spinlock::Spinlock;

/// /proc/sys/kernel/hostname — Linux __NEW_UTS_LEN = 64 (65 with NUL).
pub static HOSTNAME: Spinlock<alloc::vec::Vec<u8>> = Spinlock::new(alloc::vec::Vec::new());

/// /proc/sys/kernel/pid_max — Linux default 32768 (riscv64 max 4194304).
/// P2: stored in the PID allocator (process::pid) and consulted by
/// alloc_pid — the sysctl file is the read/write face of the live value.
pub use crate::process::pid::{pid_max_live as read_pid_max, set_pid_max_live};
pub const PID_MAX_MIN: u32 = 2;
pub const PID_MAX_MAX: u32 = 4 * 1024 * 1024;

/// /proc/sys/vm/overcommit_memory — 0 heuristic / 1 always / 2 strict.
/// Stored only: the page allocator does not consult it (no overcommit
/// accounting exists); recorded divergence from Linux.
pub static OVERCOMMIT_MEMORY: AtomicU32 = AtomicU32::new(0);

/// /proc/sys/fs/file-max — system-wide open-file ceiling. Stored only:
/// enforcement is per-process RLIMIT_NOFILE today (no global file table
/// accounting); recorded divergence.
pub static FILE_MAX: AtomicU64 = AtomicU64::new(64 * 1024);

// ============================================================================
// Generators (read side)
// ============================================================================

pub fn generate_hostname() -> Vec<u8> {
    let hn = HOSTNAME.lock();
    let mut out = hn.clone();
    out.push(b'\n');
    out
}

pub fn generate_pid_max() -> Vec<u8> {
    alloc::format!("{}\n", read_pid_max()).into_bytes()
}

pub fn generate_ostype() -> Vec<u8> {
    Vec::from(&b"Linux\n"[..])
}

pub fn generate_overcommit_memory() -> Vec<u8> {
    alloc::format!("{}\n", OVERCOMMIT_MEMORY.load(Ordering::Relaxed)).into_bytes()
}

pub fn generate_file_max() -> Vec<u8> {
    alloc::format!("{}\n", FILE_MAX.load(Ordering::Relaxed)).into_bytes()
}

// ============================================================================
// Writers (write side) — return 0 on success, negative errno on failure
// ============================================================================

/// Parse a decimal integer from a sysctl write payload (trailing '\n'
/// tolerated; empty is EINVAL).
fn parse_u64(input: &[u8]) -> Result<u64, i32> {
    let s: &[u8] = if input.last() == Some(&b'\n') {
        &input[..input.len() - 1]
    } else {
        input
    };
    if s.is_empty() || s.len() > 10 {
        return Err(-(crate::errno::constants::EINVAL as i32));
    }
    let mut v: u64 = 0;
    for &b in s {
        if !b.is_ascii_digit() {
            return Err(-(crate::errno::constants::EINVAL as i32));
        }
        v = v.checked_mul(10).and_then(|x| x.checked_add((b - b'0') as u64))
            .ok_or(-(crate::errno::constants::EINVAL as i32))?;
    }
    Ok(v)
}

pub fn write_hostname(input: &[u8]) -> i32 {
    // Linux: ≤ __NEW_UTS_LEN (64) bytes, NUL not stored, empty allowed
    // (sets the hostname to ""). Trailing newline is accepted by sysctl(8).
    let s: &[u8] = if input.last() == Some(&b'\n') {
        &input[..input.len() - 1]
    } else {
        input
    };
    if s.len() > 64 {
        return -(crate::errno::constants::EINVAL as i32);
    }
    *HOSTNAME.lock() = Vec::from(s);
    0
}

pub fn write_pid_max(input: &[u8]) -> i32 {
    match parse_u64(input) {
        Ok(v) => {
            if v < PID_MAX_MIN as u64 || v > PID_MAX_MAX as u64 {
                return -(crate::errno::constants::EINVAL as i32);
            }
            // P2: the value is live in the PID allocator; alloc_pid
            // clamps it to the bitmap capacity (32768) at use time.
            set_pid_max_live(v as u32);
            0
        }
        Err(e) => e,
    }
}

pub fn write_overcommit_memory(input: &[u8]) -> i32 {
    match parse_u64(input) {
        Ok(v) => {
            if v > 2 {
                return -(crate::errno::constants::EINVAL as i32);
            }
            OVERCOMMIT_MEMORY.store(v as u32, Ordering::Release);
            0
        }
        Err(e) => e,
    }
}

pub fn write_file_max(input: &[u8]) -> i32 {
    match parse_u64(input) {
        Ok(v) => {
            FILE_MAX.store(v, Ordering::Release);
            0
        }
        Err(e) => e,
    }
}
