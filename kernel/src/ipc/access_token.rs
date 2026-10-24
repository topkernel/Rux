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
///
/// STOPGAP (b113-b116, revised b5→b6): this image's init DOES register
/// real tokens — 85 services called ACCESS_TOKENID_SET_TOKENID on the b5
/// boot (foundation pid 852 → 0x28075a4a). The synthetic per-pid fallback
/// remains only for processes that never registered (early kthreads,
/// host-shell stragglers); it is now addressed by TGID because the ioctl
/// path keys by tgid, so it is at least stable per-process. If the
/// image's userspace sets real tokens for everything, registered values
/// always take precedence.
pub fn token_of(pid: u32) -> (u64, u64) {
    let g = REGISTRY.lock_irqsave();
    let registered = g.as_ref().and_then(|r| r.by_pid.get(&pid)).copied();
    drop(g);
    match registered {
        Some(t) => (t.token, t.ftoken),
        None => {
            // Layout verified against THIS image (b116): samgr requires
            // AccessTokenKit::GetTokenTypeFlag(token) == 1 (TOKEN_NATIVE)
            // before AddSystemAbility; libaccesstoken_sdk.z.so decodes it
            // as (token >> 27) & 3 (objdump @0x1a4df: shr 0x1b; and 3).
            // Real native tokens from the image's nativetoken.json are
            // all 0x28xxxxxx: type bits 27-28 = 0b01 (NATIVE), bit 29
            // set, version bits 24-26 = 0, uniqueid (bits 0-19) = pid.
            // The previous 0x14xxxxxx guess put bit 28 inside the type
            // field -> read as 2 (TOKEN_SHELL) -> PERMISSION DENIED
            // ("CanRequest callingTkid:..., tokenType:2" on every boot).
            let synthetic: u64 = 0x2800_0000 | (pid as u64 & 0xF_FFFF);
            (synthetic, 0)
        }
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

/// Mirror a SET_TOKENID/SET_FTOKENID from the legacy
/// drivers/access_tokenid ioctl path into the shared registry so both
/// entry points stay coherent (single store; the devfs node points at
/// this registry, but anything still holding the old ops must not
/// diverge from what BINDER_GET_SENDER_INFO reports).
pub fn mirror_set(pid: u32, set_ftoken: bool, v: u64) {
    set_token(pid, set_ftoken, v);
}

/// The REGISTERED token of a process (None when nothing ever called
/// SET_TOKENID for it) — fork inheritance copies only real tokens so
/// the synthetic per-pid fallback stays per-pid.
pub fn registered_token_of(pid: u32) -> Option<(u64, u64)> {
    let g = REGISTRY.lock_irqsave();
    g.as_ref().and_then(|r| r.by_pid.get(&pid)).map(|t| (t.token, t.ftoken))
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

    // Key the registry by TGID, not the ioctl caller's tid. Tokens are
    // per-PROCESS: init's service child calls SET between fork and exec
    // (single-threaded, tid == tgid), but libbinder's GetSelfTokenID()
    // fires from WORKER threads (IPCSkeleton on an in-process call —
    // e.g. AMS thread 856 inside foundation, tgid 852). With the tid the
    // lookup missed the 852 entry, fell into the synthetic fallback
    // (0x28000000|856) and ATM denied GET_BUNDLE_INFO for a token it
    // never allocated — StartHighestPriorityAbility's launcher query
    // then retried forever (b5). Linux's drivers/accesstokenid keys by
    // task_struct of the process; mirror the drivers/access_tokenid.rs
    // port which already uses task.tgid().
    let pid = crate::sched::current()
        .map(|t| unsafe { (*t).tgid() as u32 })
        .unwrap_or_else(crate::process::current_pid);
    // dfx=syscalltrace: the per-ioctl trace. Ungated this is the
    // highest-rate print of an OH boot — hdf_devmgr polls
    // GET_PERMISSION/GET_TOKENID in a tight loop from early boot on
    // (24k+ lines in boot b139), and hilogd.rd_kmsg then copies every
    // line from /dev/kmsg into /data logs — each batch an ext4 write
    // under the BIGLOCK, feeding the very contention the poll waits on
    // (same failure shape as the binder GET_SENDER_INFO trace, b128).
    if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::SyscallTrace) {
        crate::pr_info!("atid: pid {} ioctl {:#x}", pid, request);
    }

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
            // SET is a once-per-service event — keep it visible (it is
            // the bring-up signal that a service cleared the token gate).
            crate::pr_info!("atid: pid {} set token {:#x} (f={})", pid, v, request == ACCESS_TOKENID_SET_FTOKENID);
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
            // OH ABI (drivers/accesstokenid access_tokenid.c): the perm
            // ioctls carry their verdict in the RETURN VALUE, not only in
            // the struct: get_permission returns the permission BIT (0 or
            // 1), -ENODATA when the token has no record, -EINVAL when
            // op_code exceeds the 64-word bitmap. libtoken_setproc's
            // GetPermissionFromKernel computes isGranted = (ret == 1) and
            // falls back to the ATM service only when ret is NEGATIVE —
            // returning plain 0 for a granted bit (this port's old
            // behavior, b5-b9) made every kernel fast-path verify read as
            // DENIED, which is what kept ImplicitQueryInfoByPriority's
            // GET_BUNDLE_INFO verify failing while the bitmap actually
            // granted it (AMS retried the launcher query forever).
            // set_permission likewise returns -ENODATA instead of
            // creating an entry. One deliberate deviation: OH gates
            // ADD/REMOVE/SET on uid == 3020 (the standalone atm process);
            // this image's accesstokenmanager is hosted INSIDE foundation
            // (uid 5523), so the gate would starve the bitmap — pushes
            // stay permissive here.
            let word = (op / 32) as usize;
            if word >= 64 {
                return -22i64; // EINVAL
            }
            if request == ACCESS_TOKENID_SET_PERMISSION {
                // ioctl_set_get_perm_data.is_granted: bool at offset 8.
                let granted = buf[8] != 0;
                // dfx=syscalltrace forensics (b8): which opcode the
                // verifier's compiled g_permList computes vs the one
                // ATM's pusher set.
                if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::SyscallTrace) {
                    crate::pr_info!(
                        "atid: pid {} SET_PERM token {:#x} op {} granted {}",
                        pid,
                        token,
                        op,
                        granted
                    );
                }
                let mut g = REGISTRY.lock_irqsave();
                let r = g.get_or_insert_with(empty);
                match r.perms.get_mut(&token) {
                    Some(e) => {
                        if granted {
                            e[word] |= 1u32 << (op % 32);
                        } else {
                            e[word] &= !(1u32 << (op % 32));
                        }
                        0
                    }
                    None => -61i64, // ENODATA: no record to update
                }
            } else {
                let granted = {
                    let g = REGISTRY.lock_irqsave();
                    match g.as_ref().and_then(|r| r.perms.get(&token)) {
                        Some(p) => (p[word] >> (op % 32)) & 1,
                        None => return -61i64, // ENODATA -> userspace falls back to ATM
                    }
                };
                if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::SyscallTrace) {
                    crate::pr_info!(
                        "atid: pid {} GET_PERM token {:#x} op {} -> granted {}",
                        pid,
                        token,
                        op,
                        granted
                    );
                }
                // OH parity: the verdict rides the return value only — the
                // driver never copies the struct back (libtoken_setproc
                // discards the field and reads ret).
                granted as i64
            }
        }
        _ => {
            // dfx=syscalltrace: same tight-loop economics as the entry
            // trace (4k+ of these per boot — hilogd probing TIOCGETD on
            // the device fd).
            if crate::dfx::switches::enabled(crate::dfx::switches::DfxSwitch::SyscallTrace) {
                crate::pr_info!("access_token_id: unhandled ioctl {:#x}", request);
            }
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
    crate::pr_info!("atid: /dev/access_token_id registered");
}
