//! access_tokenid — the OpenHarmony per-process access-token registry
//! (`/dev/access_token_id`, drivers/accesstokenid in the OH kernel).
//!
//! OH's IPC security chain depends on it:
//!
//! 1. init reads /data/service/el0/access_token/nativetoken.json and every
//!    service child calls ACCESS_TOKENID_SET_TOKENID on this device between
//!    fork and exec, storing its 64-bit native token id in the kernel.
//! 2. libbinder's GetSelfTokenID() reads it back (ACCESS_TOKENID_GET_TOKENID)
//!    on every binder open.
//! 3. When samgr services a transaction, it asks the BINDER driver for the
//!    SENDER's token (BINDER_GET_SENDER_INFO, see binder.rs). samgr's
//!    CanRequest() then requires the token's type bits to say TOKEN_NATIVE —
//!    a uid-substituted "token" (what a driver without this registry
//!    effectively reports) is not native, so every AddSystemAbility is
//!    rejected with ERR_PERMISSION_DENIED (= 1).
//!
//! That rejection was the display blocker: hdf_devmgr retried
//! AddSystemAbility(SA 5100 "device_service_manager") ~100 times, exited 1,
//! the devhosts never received their driver assignments, composer_host never
//! loaded libdisplay_composer_driver_1.0.z.so, and the boot sat at a black
//! screen with every graphics process idle (boots b98-b112).
//!
//! Registry: keyed by TGID (tokens are per-process; the setter is the
//! service child itself between fork and exec, and tgid survives exec).
//! The ADD/REMOVE/GET/SET PERMISSIONS ioctls keep a per-token u32[64] bit
//! array (io control only; samgr consults the token's type bits, not this
//! array, for CanRequest).

use alloc::collections::BTreeMap;

use crate::fs::devfs::registry;
use crate::fs::dev_t::DevNo;
use crate::fs::FileOps;
use crate::sync::spinlock::Spinlock;

// ioctl numbers — _IOR/_IOW('A', nr, ...) with the exact sizes from
// OH's drivers/accesstokenid/access_tokenid.h.
const fn ioc(dir: u32, nr: u32, size: u32) -> u32 {
    (dir << 30) | (size << 16) | (b'A' as u32) << 8 | nr
}
const ACCESS_TOKENID_GET_TOKENID: u32 = ioc(2, 1, 8);
const ACCESS_TOKENID_SET_TOKENID: u32 = ioc(1, 2, 8);
const ACCESS_TOKENID_GET_FTOKENID: u32 = ioc(2, 3, 8);
const ACCESS_TOKENID_SET_FTOKENID: u32 = ioc(1, 4, 8);
const ACCESS_TOKENID_ADD_PERMISSIONS: u32 = ioc(1, 5, 260);
const ACCESS_TOKENID_REMOVE_PERMISSIONS: u32 = ioc(1, 6, 4);
const ACCESS_TOKENID_GET_PERMISSION: u32 = ioc(1, 7, 12);
const ACCESS_TOKENID_SET_PERMISSION: u32 = ioc(1, 8, 12);

const ACCESS_TOKEN_MINOR: u32 = 0xB7;

pub const DEV_ACCESS_TOKEN: DevNo = DevNo::new(crate::fs::dev_t::MISC_MAJOR, ACCESS_TOKEN_MINOR);

#[derive(Clone, Copy, Default)]
struct TokenInfo {
    token: u64,
    ftoken: u64,
}

struct Registry {
    by_pid: BTreeMap<u32, TokenInfo>,
    perms: BTreeMap<u32, [u32; 64]>,
}

static REGISTRY: Spinlock<Option<Registry>> = Spinlock::new(None);

fn empty() -> Registry {
    Registry {
        by_pid: BTreeMap::new(),
        perms: BTreeMap::new(),
    }
}

/// (token, first token) of a process; (0, 0) when never set. Used by the
/// binder driver's BINDER_GET_ACCESS_TOKEN / BINDER_GET_SENDER_INFO to
/// report senders to samgr.
pub fn token_of(pid: u32) -> (u64, u64) {
    let g = REGISTRY.lock_irqsave();
    match g.as_ref().and_then(|r| r.by_pid.get(&pid)) {
        Some(t) => (t.token, t.ftoken),
        None => (0, 0),
    }
}

fn set_token(pid: u32, set_ftoken: bool, v: u64) {
    let mut g = REGISTRY.lock_irqsave();
    let r = g.get_or_insert_with(empty);
    let e = r.by_pid.entry(pid).or_default();
    if set_ftoken {
        e.ftoken = v;
    } else {
        e.token = v;
    }
}

fn ebadf() -> i64 {
    crate::errno::Errno::BadAddress.as_neg_i32() as i64
}

fn eperm() -> i64 {
    crate::errno::Errno::PermissionDenied.as_neg_i32() as i64
}

fn enotty() -> i64 {
    crate::errno::Errno::NotATypewriter.as_neg_i32() as i64
}

fn access_token_ioctl(request: u32, arg: usize) -> i64 {
    use crate::arch::uaccess::{access_ok, copy_from_user, copy_to_user};

    let pid = unsafe { crate::process::current_pid() };

    match request {
        ACCESS_TOKENID_GET_TOKENID | ACCESS_TOKENID_GET_FTOKENID => {
            if arg == 0 || !access_ok(arg, 8) {
                return ebadf();
            }
            let (token, ftoken) = token_of(pid);
            let v = if request == ACCESS_TOKENID_GET_TOKENID {
                token
            } else {
                ftoken
            };
            // SAFETY: arg validated, 8 bytes.
            if unsafe { copy_to_user(arg as *mut u8, v.to_ne_bytes().as_ptr(), 8) } != 0 {
                return ebadf();
            }
            0
        }
        ACCESS_TOKENID_SET_TOKENID | ACCESS_TOKENID_SET_FTOKENID => {
            if arg == 0 || !access_ok(arg, 8) {
                return ebadf();
            }
            let mut buf = [0u8; 8];
            // SAFETY: arg validated, 8 bytes.
            if unsafe { copy_from_user(buf.as_mut_ptr(), arg as *const u8, 8) } != 0 {
                return ebadf();
            }
            let v = u64::from_ne_bytes(buf);
            // OH requires root / the device owner (uid 3020) /
            // nwebspawn+render. We accept root and 3020 — init's service
            // children run as root until their setuid.
            let uid = crate::process::current_task()
                .map(|t| unsafe { (*t).cred().uid })
                .unwrap_or(0);
            if uid != 0 && uid != 3020 {
                return eperm();
            }
            set_token(pid, request == ACCESS_TOKENID_SET_FTOKENID, v);
            0
        }
        ACCESS_TOKENID_ADD_PERMISSIONS => {
            if arg == 0 || !access_ok(arg, 260) {
                return ebadf();
            }
            let mut buf = [0u8; 260];
            // SAFETY: arg validated, 260 bytes.
            if unsafe { copy_from_user(buf.as_mut_ptr(), arg as *const u8, 260) } != 0 {
                return ebadf();
            }
            let token = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]);
            let mut g = REGISTRY.lock_irqsave();
            let r = g.get_or_insert_with(empty);
            let e = r.perms.entry(token).or_insert([0u32; 64]);
            for i in 0..64 {
                let off = 4 + i * 4;
                let word = u32::from_ne_bytes([
                    buf[off],
                    buf[off + 1],
                    buf[off + 2],
                    buf[off + 3],
                ]);
                e[i] |= word;
            }
            0
        }
        ACCESS_TOKENID_REMOVE_PERMISSIONS => {
            if arg == 0 || !access_ok(arg, 4) {
                return ebadf();
            }
            let mut buf = [0u8; 4];
            // SAFETY: arg validated, 4 bytes.
            if unsafe { copy_from_user(buf.as_mut_ptr(), arg as *const u8, 4) } != 0 {
                return ebadf();
            }
            let token = u32::from_ne_bytes(buf);
            let mut g = REGISTRY.lock_irqsave();
            if let Some(r) = g.as_mut() {
                r.perms.remove(&token);
            }
            0
        }
        ACCESS_TOKENID_GET_PERMISSION | ACCESS_TOKENID_SET_PERMISSION => {
            if arg == 0 || !access_ok(arg, 12) {
                return ebadf();
            }
            let mut buf = [0u8; 12];
            // SAFETY: arg validated, 12 bytes.
            if unsafe { copy_from_user(buf.as_mut_ptr(), arg as *const u8, 12) } != 0 {
                return ebadf();
            }
            let token = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]);
            let op = u32::from_ne_bytes([buf[4], buf[5], buf[6], buf[7]]);
            if request == ACCESS_TOKENID_SET_PERMISSION {
                // ioctl_set_get_perm_data.is_granted: bool at offset 8.
                let granted = buf[8] != 0;
                let mut g = REGISTRY.lock_irqsave();
                let r = g.get_or_insert_with(empty);
                let e = r.perms.entry(token).or_insert([0u32; 64]);
                let word = (op / 32) as usize;
                if word < 64 {
                    if granted {
                        e[word] |= 1u32 << (op % 32);
                    } else {
                        e[word] &= !(1u32 << (op % 32));
                    }
                }
                0
            } else {
                let granted = {
                    let g = REGISTRY.lock_irqsave();
                    g.as_ref()
                        .and_then(|r| r.perms.get(&token))
                        .map(|p| {
                            p.get((op / 32) as usize)
                                .map_or(0, |w| (*w >> (op % 32)) & 1)
                                != 0
                        })
                        .unwrap_or(false)
                };
                buf[8] = granted as u8;
                buf[9] = 0;
                buf[10] = 0;
                buf[11] = 0;
                // SAFETY: arg validated, 12 bytes.
                if unsafe { copy_to_user(arg as *mut u8, buf.as_ptr(), 12) } != 0 {
                    return ebadf();
                }
                0
            }
        }
        _ => {
            crate::pr_info!("access_token_id: unhandled ioctl {:#x}", request);
            enotty()
        }
    }
}

/// Dispatch entry from sys_ioctl (None when the file is not ours; ops
/// identity check — evdev/binder pattern).
pub fn access_token_file_ioctl(file: &crate::fs::File, request: u32, arg: usize) -> Option<i64> {
    let ops = file.get_ops()?;
    if !core::ptr::eq(ops as *const _, &ACCESS_TOKEN_OPS as *const _) {
        return None;
    }
    Some(access_token_ioctl(request, arg))
}

fn at_close(_file: &crate::fs::File) -> i32 {
    0
}

fn at_poll(_file: &crate::fs::File, _events: u16) -> u16 {
    crate::syscall::misc::poll_events::POLLIN | crate::syscall::misc::poll_events::POLLRDNORM
}

pub static ACCESS_TOKEN_OPS: FileOps = FileOps {
    read: None,
    write: None,
    lseek: None,
    close: Some(at_close),
    poll: Some(at_poll),
    read_at: None,
};

/// Register the char device (called from devfs init, next to binder).
pub fn register() {
    let _ = registry::register_char_device(DEV_ACCESS_TOKEN, &ACCESS_TOKEN_OPS);
}
