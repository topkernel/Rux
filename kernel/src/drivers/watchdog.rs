//! /dev/watchdog — minimal Linux-parity soft watchdog character device.
//!
//! Consumers on the OH image:
//! - `watchdog_service` (base/startup/init/watchdog/init_watchdog.c):
//!   WaitAtStartup(stat("/dev/watchdog") == 0, 10s) -> open(O_RDWR) ->
//!   WDIOC_SETTIMEOUT(interval+margin) -> WDIOC_GETTIMEOUT ->
//!   [WDIOC_SETPRETIMEOUT/GETPRETIMEOUT] -> loop { WDIOC_KEEPALIVE }.
//!   Without the node the service waits 10s, logs "wait for file
//!   failed" / "Can't open /dev/watchdog" and dies (b151: pid 306
//!   zombie). It is the only consumer in the current image.
//!
//! Semantics: a non-firing watchdog. The ioctls store/report state so
//! userspace sees a real softdog, but no timer ever reboots the machine
//! — during bring-up a real firing watchdog would turn every hang under
//! investigation into a reboot that destroys the evidence. When a
//! firing behavior is wanted, arm it behind a boot flag.
//!
//! Device number: misc 10:130 (Linux WATCHDOG_MINOR).

use crate::fs::dev_t::{DevNo, MISC_MAJOR};
use core::sync::atomic::{AtomicU32, Ordering};

/// /dev/watchdog device number (misc 10:130, Linux WATCHDOG_MINOR).
pub const DEV_WATCHDOG: DevNo = DevNo::new(MISC_MAJOR, 130);

// ---- uapi (include/uapi/linux/watchdog.h) --------------------------------

/// struct watchdog_info: u32 options, u32 firmware_version, u8[32] identity.
const WDIOC_GETSUPPORT: u32 = 0x8028_5700; // _IOR('W', 0, struct watchdog_info)
const WDIOC_GETSTATUS: u32 = 0x8004_5701; // _IOR('W', 1, int)
const WDIOC_GETBOOTSTATUS: u32 = 0x8004_5702; // _IOR('W', 2, int)
const WDIOC_SETOPTIONS: u32 = 0x8004_5704; // _IOR('W', 4, int)
const WDIOC_KEEPALIVE: u32 = 0x0000_5705; // _IO('W', 5)
const WDIOC_SETTIMEOUT: u32 = 0xc004_5706; // _IOWR('W', 6, int)
const WDIOC_GETTIMEOUT: u32 = 0x8004_5707; // _IOR('W', 7, int)
const WDIOC_SETPRETIMEOUT: u32 = 0xc004_5708; // _IOWR('W', 8, int)
const WDIOC_GETPRETIMEOUT: u32 = 0x8004_5709; // _IOR('W', 9, int)
const WDIOC_GETTIMELEFT: u32 = 0x8004_570a; // _IOR('W', 10, int)

/// WDIOF_* capability bits we report (SETTIMEOUT | KEEPALIVEPING |
/// MAGICCLOSE; no card-reset status bits — the device never fires).
const WDIOF_CAPS: u32 = 0x2 | 0x4 | 0x8;

/// WDIOS_DISABLECARD / WDIOS_ENABLECARD (SETOPTIONS argument bits).
const WDIOS_DISABLECARD: i32 = 0x1;
const WDIOS_ENABLECARD: i32 = 0x2;

/// Reported identity (same string the kernel softdog uses).
const IDENTITY: &[u8] = b"softdog";

// ---- state ---------------------------------------------------------------

/// Configured timeout in seconds (Linux softdog default is 19; any
/// positive value round-trips — the OH caller always sets its own
/// before reading, and we never fire).
static TIMEOUT_S: AtomicU32 = AtomicU32::new(19);
/// Configured pretimeout in seconds (0 = none).
static PRETIMEOUT_S: AtomicU32 = AtomicU32::new(0);

// ---- file ops ------------------------------------------------------------

/// write() is the classic watchdog keepalive ping (any byte).
fn watchdog_write(_file: &crate::fs::file::File, buf: &[u8]) -> isize {
    if buf.is_empty() {
        return -22; // EINVAL (Linux checks the count)
    }
    buf.len() as isize
}

/// read(): no card status to report — EOF. (Linux watchdog_dev has no
/// mandatory read; the OH consumer never reads.)
fn watchdog_read(_file: &crate::fs::file::File, _buf: &mut [u8]) -> isize {
    0
}

/// /dev/watchdog file operations.
pub static WATCHDOG_OPS: crate::fs::FileOps = crate::fs::FileOps::builder()
    .read(watchdog_read)
    .write(watchdog_write)
    .build();

/// True when `file` is an open /dev/watchdog description.
pub fn is_watchdog_file(file: &crate::fs::file::File) -> bool {
    match file.get_ops() {
        Some(ops) => core::ptr::eq(ops as *const _, &WATCHDOG_OPS as *const _),
        None => false,
    }
}

/// Per-fd ioctl dispatch (ops identity). Returns Some(ret) when the file
/// is the watchdog device, None otherwise.
pub fn watchdog_file_ioctl(
    file: &crate::fs::file::File,
    request: u32,
    arg: usize,
) -> Option<i64> {
    if !is_watchdog_file(file) {
        return None;
    }
    Some(unsafe { watchdog_ioctl(request, arg) })
}

// ---- ioctls --------------------------------------------------------------

/// Copy one int in from userspace.
///
/// # Safety
/// `arg` must be a userspace pointer with 4 readable bytes.
unsafe fn get_int(arg: usize) -> Option<i32> {
    let mut b = [0u8; 4];
    if crate::arch::uaccess::copy_from_user(b.as_mut_ptr(), arg as *const u8, 4) != 0 {
        return None;
    }
    Some(i32::from_ne_bytes(b))
}

/// Copy one int out to userspace.
///
/// # Safety
/// `arg` must be a userspace pointer with 4 writable bytes.
unsafe fn put_int(arg: usize, v: i32) -> i64 {
    if crate::arch::uaccess::copy_to_user(arg as *mut u8, v.to_ne_bytes().as_ptr(), 4) != 0 {
        return -14; // EFAULT
    }
    0
}

/// # Safety
/// `arg` semantics per request (uapi pointer or ignored).
unsafe fn watchdog_ioctl(request: u32, arg: usize) -> i64 {
    match request {
        WDIOC_GETSUPPORT => {
            // struct watchdog_info: options, firmware_version, identity[32]
            let mut wi = [0u8; 40];
            wi[0..4].copy_from_slice(&WDIOF_CAPS.to_ne_bytes());
            // firmware_version stays 0
            let n = IDENTITY.len().min(31);
            wi[8..8 + n].copy_from_slice(&IDENTITY[..n]);
            if crate::arch::uaccess::copy_to_user(arg as *mut u8, wi.as_ptr(), 40) != 0 {
                return -14; // EFAULT
            }
            0
        }
        WDIOC_GETSTATUS | WDIOC_GETBOOTSTATUS => {
            // No card-reset bits: the watchdog never fired.
            put_int(arg, 0)
        }
        WDIOC_SETOPTIONS => {
            match get_int(arg) {
                Some(opts) if opts & !(WDIOS_DISABLECARD | WDIOS_ENABLECARD) == 0 => 0,
                Some(_) => -22, // EINVAL: unknown option bits
                None => -14,    // EFAULT
            }
        }
        WDIOC_KEEPALIVE => 0,
        WDIOC_SETTIMEOUT => match get_int(arg) {
            Some(t) if t > 0 && t <= 65535 => {
                TIMEOUT_S.store(t as u32, Ordering::Relaxed);
                put_int(arg, t)
            }
            Some(_) => -22, // EINVAL (Linux clamps to driver bounds; 0 is rejected)
            None => -14,    // EFAULT
        },
        WDIOC_GETTIMEOUT => put_int(arg, TIMEOUT_S.load(Ordering::Relaxed) as i32),
        WDIOC_SETPRETIMEOUT => match get_int(arg) {
            Some(t) if t >= 0 && t <= 65535 => {
                PRETIMEOUT_S.store(t as u32, Ordering::Relaxed);
                put_int(arg, t)
            }
            Some(_) => -22,
            None => -14,
        },
        WDIOC_GETPRETIMEOUT => put_int(arg, PRETIMEOUT_S.load(Ordering::Relaxed) as i32),
        // We never fire, so the full timeout is always left.
        WDIOC_GETTIMELEFT => put_int(arg, TIMEOUT_S.load(Ordering::Relaxed) as i32),
        _ => -25, // ENOTTY
    }
}
