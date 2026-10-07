//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /dev/access_token_id — OpenHarmony access-token bookkeeping device.
//!
//! Skeleton port of the OH kernel patch (OpenHarmony-5.10/6.6
//! `drivers/accesstokenid`, patch "[Kernel] accesstokenid: tokendid is
//! used for special app security control"): a misc char device exposing
//! per-process token bookkeeping to the OH access-token manager (ATM) —
//! the most-referenced device node in the reference OH boot log.
//!
//! UAPI (all payloads are `unsigned long long`, magic 'A'):
//!   ACCESS_TOKENID_GET_TOKENID  _IOR('A', 1, u64) = 0x80084101
//!   ACCESS_TOKENID_SET_TOKENID  _IOW('A', 2, u64) = 0x40084102
//!   ACCESS_TOKENID_GET_FTOKENID _IOR('A', 3, u64) = 0x80084103
//!   ACCESS_TOKENID_SET_FTOKENID _IOW('A', 4, u64) = 0x40084104
//!
//! Semantics (per the patch, to be calibrated against real OH userspace
//! in Phase 2 of the port plan):
//! - each task carries `token` and `ftoken` (u64 each);
//! - fork: child inherits `token`, `ftoken` is cleared;
//! - SET_* requires privilege (the upstream driver checks
//!   capable(CAP_SYS_ADMIN) or uid match against the device inode owner;
//!   this skeleton allows uid 0 / CAP_SYS_ADMIN);
//! - GET_* is unrestricted.
//!
//! /proc/<pid>/tokenid exposure is not implemented (skeleton scope).

use crate::fs::dev_t::{DevNo, MISC_MAJOR};

/// ioctl command words (asm-generic encoding, 8-byte u64 payload).
const ACCESS_TOKENID_GET_TOKENID: u32 = 0x8008_4101; // _IOR('A', 1, u64)
const ACCESS_TOKENID_SET_TOKENID: u32 = 0x4008_4102; // _IOW('A', 2, u64)
const ACCESS_TOKENID_GET_FTOKENID: u32 = 0x8008_4103; // _IOR('A', 3, u64)
const ACCESS_TOKENID_SET_FTOKENID: u32 = 0x4008_4104; // _IOW('A', 4, u64)

/// /dev/access_token_id device number (misc 10:63; upstream uses a
/// dynamically allocated misc minor — userspace opens by path).
pub const DEV_ACCESS_TOKENID: DevNo = DevNo::new(MISC_MAJOR, 63);

fn tokenid_read(_file: &crate::fs::file::File, _buf: &mut [u8]) -> isize {
    -22 // EINVAL: not readable (upstream has no .read)
}

fn tokenid_write(_file: &crate::fs::file::File, _buf: &[u8]) -> isize {
    -22 // EINVAL: not writable
}

/// /dev/access_token_id file operations.
pub static TOKENID_OPS: crate::fs::FileOps = crate::fs::FileOps {
    read: Some(tokenid_read),
    write: Some(tokenid_write),
    lseek: None,
    close: None,
    poll: None,
};

/// True when `file` is an open /dev/access_token_id description.
pub fn is_tokenid_file(file: &crate::fs::file::File) -> bool {
    match file.get_ops() {
        Some(ops) => core::ptr::eq(ops as *const _, &TOKENID_OPS as *const _),
        None => false,
    }
}

/// Per-fd ioctl dispatch (ops identity). Returns Some(ret) when the file
/// is the tokenid device, None otherwise.
pub fn tokenid_file_ioctl(
    file: &crate::fs::file::File,
    request: u32,
    arg: usize,
) -> Option<i64> {
    if !is_tokenid_file(file) {
        return None;
    }
    Some(unsafe { tokenid_ioctl(request, arg) })
}

/// SAFETY: `arg` is the raw syscall argument; the u64 payload pointer is
/// validated through uaccess before dereference.
unsafe fn tokenid_ioctl(request: u32, arg: usize) -> i64 {
    use crate::arch::uaccess::{access_ok, get_user, put_user};

    const EINVAL: i64 = 22;
    const ENOTTY: i64 = 25;
    const EPERM: i64 = 1;
    const EFAULT: i64 = 14;

    // Upstream rejects a NULL payload for every command.
    if arg == 0 || !access_ok(arg, 8) {
        return -EINVAL;
    }

    // Fetch/prepare the 8-byte payload (the upstream handler reads the
    // u64 first for every command, even GETs).
    let mut value: u64 = 0;
    if request == ACCESS_TOKENID_SET_TOKENID || request == ACCESS_TOKENID_SET_FTOKENID {
        // SAFETY: arg validated for 8 readable bytes.
        match get_user(arg as *const u64) {
            Some(v) => value = v,
            None => return -EFAULT,
        }
    }

    let task = match crate::sched::current() {
        Some(t) => t,
        None => return -EINVAL,
    };

    match request {
        ACCESS_TOKENID_GET_TOKENID => {
            // SAFETY: arg validated for 8 writable bytes.
            if !put_user(arg as *mut u64, task.access_token()) {
                return -EFAULT;
            }
            0
        }
        ACCESS_TOKENID_SET_TOKENID => {
            if !set_allowed() {
                return -EPERM;
            }
            task.set_access_token(value);
            0
        }
        ACCESS_TOKENID_GET_FTOKENID => {
            // SAFETY: arg validated for 8 writable bytes.
            if !put_user(arg as *mut u64, task.access_ftoken()) {
                return -EFAULT;
            }
            0
        }
        ACCESS_TOKENID_SET_FTOKENID => {
            if !set_allowed() {
                return -EPERM;
            }
            task.set_access_ftoken(value);
            0
        }
        _ => -ENOTTY,
    }
}

/// SET permission: uid 0 or CAP_SYS_ADMIN (the upstream driver checks
/// the device inode owner uid; every caller of record — init, appspawn —
/// runs as root, so the simplified check matches observed usage).
fn set_allowed() -> bool {
    if crate::security::capable(crate::security::CAP_SYS_ADMIN) {
        return true;
    }
    crate::sched::current()
        .map(|t| t.cred().euid == 0)
        .unwrap_or(false)
}
