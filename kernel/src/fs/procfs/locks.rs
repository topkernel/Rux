//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/locks — the held file locks (Linux's fs/locks.c).
//!
//! Linux's format, one lock per line:
//!
//! ```text
//! 1: POSIX  ADVISORY  WRITE 28848 08:12:76022103 0 EOF
//! 2: -> POSIX  ADVISORY  READ 9833 08:12:73924810 128 128
//! ```
//!
//! The columns are: the lock id, the class (POSIX record lock, FLOCK or
//! OFDLCK), the type (ADVISORY or MANDATORY), the mode (READ or WRITE),
//! the owner's pid, the device and inode, and the first and last byte of
//! the locked range ("0 EOF" for an unbounded range). A "->" prefix
//! marks a lock request that is blocked, waiting for the holder.
//!
//! The device field is "00:00:<inode>": Rux's inodes carry a filesystem
//! id rather than a device number, and the rest of Rux's procfs (the
//! maps line) reports the same "00:00" device.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Generate /proc/locks content.
pub fn generate() -> Vec<u8> {
    render().into_bytes()
}

/// The same list, as text (Linux's sysrq 'd' prints the /proc/locks
/// rows verbatim).
pub fn render() -> alloc::string::String {
    let mut out = String::new();
    for (id, lock) in crate::fs::locks::snapshot_locks().iter().enumerate() {
        // Column layout from Linux fs/locks.c lock_get_status:
        // "POSIX " + " ADVISORY " + "WRITE " + "pid ..." — i.e. two
        // spaces between class and type and between type and mode, one
        // space before the pid. FLOCK rows are the literal
        // "FLOCK  ADVISORY  " there; both classes pad to the same
        // columns here ("POSIX "/"FLOCK " + " ADVISORY  ").
        let class = match lock.class {
            1 => "FLOCK ",
            _ => "POSIX ",
        };
        let mode = if lock.exclusive { "WRITE" } else { "READ" };
        let end = if lock.end == u64::MAX {
            "EOF".to_string()
        } else {
            format!("{}", lock.end)
        };
        out.push_str(&format!(
            "{}: {}{} ADVISORY  {} {} 00:00:{} {} {}\n",
            id + 1,
            if lock.waiting { "-> " } else { "" },
            class,
            mode,
            lock.pid,
            lock.ino,
            lock.start,
            end,
        ));
    }
    out
}
