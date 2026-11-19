//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /proc/self - Symlink to current process directory
//!
//! /proc/self is a symbolic link that points to /proc/[pid] where
//! [pid] is the PID of the process reading the link.

use alloc::vec::Vec;
use alloc::format;

/// Get the link target for /proc/self
///
/// Returns the PID number of the current process.
pub fn get_self_link() -> Vec<u8> {
    use crate::process::current_pid;
    let pid = current_pid();
    format!("{}", pid).into_bytes()
}

/// Generate /proc/self content (same as link target)
pub fn generate() -> Vec<u8> {
    get_self_link()
}

/// Get the link target for /proc/thread-self: the current THREAD's id
/// (the calling task's own pid, not the tgid). libselinux's
/// setexeccon()/setcon() open /proc/thread-self/attr/exec first (with a
/// /proc/self/task/<tid> fallback); without the link every open fails
/// ENOENT and OH's init aborts the service child before exec.
pub fn get_thread_self_link() -> Vec<u8> {
    match crate::sched::current() {
        Some(t) => {
            // SAFETY: current returns the running task's pointer.
            let tid = unsafe { (*t).pid() };
            format!("{}", tid).into_bytes()
        }
        None => format!("{}", crate::process::current_pid()).into_bytes(),
    }
}
