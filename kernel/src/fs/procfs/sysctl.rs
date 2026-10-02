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
//!   /proc/sys/kernel/core_pattern   rw (default "core.%p"; specifiers
//!                                    %p/%e/%s/%u/%g/%h/%% expanded at
//!                                    dump time, see process::coredump)
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

/// /proc/sys/kernel/hostname — Linux __NEW_UTS_LEN = 64 (65 with NUL).
/// U1c: superseded by the per-UTS-namespace storage
/// (process::ns::current_uts_ns()); retained only as the boot seed
/// compatibility symbol.

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

/// /proc/sys/fs/pipe-user-pages-soft — total pages granted to pipes
/// before unprivileged pipe allocations shrink to 1 page (Linux default
/// 16384; read by LTP pipe15). We do not enforce the limit yet; 1024
/// pages matches the fdtable scale (MAX_FDS = 1024 → 128 max pipes at
/// 64KB, well under this budget) so the derived pipe count stays creatable.
pub static PIPE_USER_PAGES_SOFT: AtomicU64 = AtomicU64::new(1024);
/// /proc/sys/fs/pipe-user-pages-hard — hard cap variant (Linux default
/// 1048576, i.e. 0 for "not set" on some kernels; the common default is
/// 1048576).
pub static PIPE_USER_PAGES_HARD: AtomicU64 = AtomicU64::new(1048576);

/// /proc/sys/kernel/core_pattern — Linux CORENAME_MAX_SIZE = 128.
pub const CORENAME_MAX_SIZE: usize = 128;

/// /proc/sys/kernel/core_pattern storage.
///
/// Default "core.%p" (Linux ships plain "core"; the pid suffix keeps
/// concurrent crash dumps from overwriting each other — the recorded
/// divergence is deliberate for bring-up debugging). A leading '|' (pipe
/// to a usermode helper) is REJECTED at write time: no helper infra
/// exists, and silently accepting it would lose every core.
pub struct CorePattern {
    buf: [u8; CORENAME_MAX_SIZE],
    len: usize,
}

impl CorePattern {
    const DEFAULT: &[u8] = b"core.%p";

    /// Const initializer seeding the default pattern.
    pub const fn new() -> Self {
        let mut buf = [0u8; CORENAME_MAX_SIZE];
        let mut i = 0;
        while i < Self::DEFAULT.len() {
            buf[i] = Self::DEFAULT[i];
            i += 1;
        }
        Self { buf, len: Self::DEFAULT.len() }
    }

    /// Current pattern bytes (no NUL, no newline).
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

/// The live core_pattern (read by process::coredump::do_coredump).
pub static CORE_PATTERN: crate::sync::Spinlock<CorePattern> =
    crate::sync::Spinlock::new(CorePattern::new());

// ============================================================================
// Generators (read side)
// ============================================================================

pub fn generate_hostname() -> Vec<u8> {
    // U1c: hostname lives in the caller's UTS namespace (global sysctl
    // surface, per-namespace storage — Linux semantic).
    let mut out = crate::process::ns::current_uts_ns().get_hostname();
    out.push(b'\n');
    out
}

pub fn generate_pid_max() -> Vec<u8> {
    alloc::format!("{}\n", read_pid_max()).into_bytes()
}

pub fn generate_ostype() -> Vec<u8> {
    Vec::from(&b"Linux\n"[..])
}

/// /proc/sys/kernel/cap_last_cap — highest supported capability number
/// (read by libcap-ng init() to size the capability bounding set).
pub fn generate_cap_last_cap() -> Vec<u8> {
    alloc::format!("{}\n", crate::security::capability::CAP_LAST_CAP).into_bytes()
}

pub fn generate_overcommit_memory() -> Vec<u8> {
    alloc::format!("{}\n", OVERCOMMIT_MEMORY.load(Ordering::Relaxed)).into_bytes()
}

pub fn generate_file_max() -> Vec<u8> {
    alloc::format!("{}\n", FILE_MAX.load(Ordering::Relaxed)).into_bytes()
}

pub fn generate_pipe_user_pages_soft() -> Vec<u8> {
    alloc::format!("{}\n", PIPE_USER_PAGES_SOFT.load(Ordering::Relaxed)).into_bytes()
}

pub fn generate_pipe_user_pages_hard() -> Vec<u8> {
    alloc::format!("{}\n", PIPE_USER_PAGES_HARD.load(Ordering::Relaxed)).into_bytes()
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
    // U1c: the write affects the CALLER's UTS namespace only.
    let s: &[u8] = if input.last() == Some(&b'\n') {
        &input[..input.len() - 1]
    } else {
        input
    };
    if s.len() > 64 {
        return -(crate::errno::constants::EINVAL as i32);
    }
    if !crate::process::ns::current_uts_ns().set_hostname(s) {
        return -(crate::errno::constants::EINVAL as i32);
    }
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

pub fn write_pipe_user_pages_soft(input: &[u8]) -> i32 {
    match parse_u64(input) {
        Ok(v) => {
            PIPE_USER_PAGES_SOFT.store(v, Ordering::Release);
            0
        }
        Err(e) => e,
    }
}

pub fn write_pipe_user_pages_hard(input: &[u8]) -> i32 {
    match parse_u64(input) {
        Ok(v) => {
            PIPE_USER_PAGES_HARD.store(v, Ordering::Release);
            0
        }
        Err(e) => e,
    }
}

/// /proc/sys/kernel/msgmni — System V message queue count limit. Matches
/// the IPC_IDS_MAX slots the ipc table offers.
pub fn generate_msgmni() -> Vec<u8> {
    alloc::format!("{}\n", crate::ipc::util::IPC_IDS_MAX).into_bytes()
}

/// /proc/sys/kernel/msgmax — largest single message (bytes).
pub fn generate_msgmax() -> Vec<u8> {
    alloc::format!("8192\n").into_bytes()
}

/// /proc/sys/kernel/msgmnb — default max queue size in bytes.
pub fn generate_msgmnb() -> Vec<u8> {
    alloc::format!("16384\n").into_bytes()
}

pub fn generate_core_pattern() -> Vec<u8> {
    let mut out = CORE_PATTERN.lock().as_bytes().to_vec();
    out.push(b'\n');
    out
}

pub fn write_core_pattern(input: &[u8]) -> i32 {
    // sysctl(8) appends '\n'; tolerate it.
    let s: &[u8] = if input.last() == Some(&b'\n') {
        &input[..input.len() - 1]
    } else {
        input
    };
    let einval = -(crate::errno::constants::EINVAL as i32);
    if s.is_empty() || s.len() > CORENAME_MAX_SIZE {
        return einval;
    }
    if s[0] == b'|' {
        // Pipe-to-helper patterns (systemd-coredump) need a usermode
        // helper; rejecting keeps the write loud instead of losing cores.
        crate::pr_warn!("core_pattern: pipe handler not supported\n");
        return einval;
    }
    let mut pat = CORE_PATTERN.lock();
    pat.buf[..s.len()].copy_from_slice(s);
    pat.len = s.len();
    0
}
