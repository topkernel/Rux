//! VT (virtual terminal) layer — /dev/tty0 master + /dev/tty1..ttyN
//! console devices, Linux `include/uapi/linux/vt.h` / `kd.h` ABI.
//!
//! The gdm chain's display-switch protocol runs entirely over these:
//! - gdm-session-worker `set_up_for_new_vt()`: open("/dev/tty1") (the
//!   "open console of initial fd" step — ENOENT killed every greeter
//!   session at SESSION_OPENED until these nodes existed), VT_OPENQRY
//!   on it, then open("/dev/ttyN") for the returned VT.
//! - `jump_to_vt()`: open("/dev/tty0"), VT_GETSTATE, VT_ACTIVATE,
//!   VT_WAITACTIVE; `fix_terminal_vt_mode()`: VT_SETMODE/VT_GETMODE
//!   (VT_PROCESS + relsig/acqsig).
//! - Xorg on the session VT: VT_ACTIVATE/VT_WAITACTIVE, VT_SETMODE,
//!   KDSETMODE(KD_GRAPHICS), KDGKBTYPE/KDGKBMODE/KDSKBMODE.
//! - logind TakeControl: same ioctls on /dev/tty0.
//!
//! Rux has one display (virtio-gpu fb + UART text console), so the VT
//! layer is a single active-VT register with in-use refcounts per VT:
//! VT_ACTIVATE moves the active VT and wakes VT_WAITACTIVE sleepers;
//! the switch never blocks on a VT_PROCESS handshake (a release/acquire
//! signal round-trip) because there is no competing console owner. The
//! read/write/poll semantics delegate to the shared console TtyDevice
//! (same sink as /dev/console).
//!
//! dev_t layout (Linux): tty0..ttyN are (4, 0..N); /dev/tty is (5, 0)
//! and /dev/console is (5, 1) (TTYAUX_MAJOR). ttyS0 remains (4, 64).

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::process::wait::WaitQueueHead;
use crate::sync::Spinlock;

/// Highest VT number this kernel provides (/dev/tty1..=tty7).
pub const NR_VT: u32 = 7;

/// vt ioctl commands (include/uapi/linux/vt.h).
const VT_OPENQRY: u32 = 0x5600;
const VT_GETMODE: u32 = 0x5601;
const VT_SETMODE: u32 = 0x5602;
const VT_GETSTATE: u32 = 0x5603;
const VT_SENDSIG: u32 = 0x5604;
const VT_RELDISP: u32 = 0x5605;
const VT_ACTIVATE: u32 = 0x5606;
const VT_WAITACTIVE: u32 = 0x5607;
const VT_DISALLOCATE: u32 = 0x5608;
const VT_RESIZE: u32 = 0x5609;
const VT_RESIZEX: u32 = 0x560A;
const VT_LOCKSWITCH: u32 = 0x560B;
const VT_UNLOCKSWITCH: u32 = 0x560C;

/// kd ioctl commands (include/uapi/linux/kd.h) — the subset gdm, Xorg
/// and logind issue on VT fds.
const KIOCSOUND: u32 = 0x4B2F;
const KDGETLED: u32 = 0x4B31;
const KDSETLED: u32 = 0x4B32;
const KDGKBTYPE: u32 = 0x4B33;
const KDSETMODE: u32 = 0x4B3A;
const KDGETMODE: u32 = 0x4B3B;
const KDGKBMODE: u32 = 0x4B44;
const KDSKBMODE: u32 = 0x4B45;

const VT_AUTO: u8 = 0x00;
const VT_PROCESS: u8 = 0x01;

const KD_TEXT: u32 = 0x00;
const KD_GRAPHICS: u32 = 0x01;

/// Keyboard type answer for KDGKBTYPE: 101-key PC (Xorg's probe).
const KB_101: u8 = 0x02;

/// struct vt_mode (8 bytes): mode, waitv, relsig, acqsig, frsig.
struct VtMode {
    mode: u8,
    waitv: u8,
    relsig: u16,
    acqsig: u16,
    frsig: u16,
}

// ============================================================================
// Global VT state
// ============================================================================

/// The active VT (starts at 1: gdm's GDM_INITIAL_VT).
static ACTIVE_VT: AtomicU32 = AtomicU32::new(1);

/// Open-fd refcount per VT number (index 0 = /dev/tty0 master view, not
/// itself allocatable by VT_OPENQRY). VT_OPENQRY skips in-use VTs so
/// two displays never share a VT.
static VT_REFS: [AtomicU32; (NR_VT as usize) + 1] =
    [const { AtomicU32::new(0) }; (NR_VT as usize) + 1];

/// Active-console vt_mode (single display → single mode register).
static VT_MODE: Spinlock<VtMode> = Spinlock::new(VtMode {
    mode: VT_AUTO,
    waitv: 0,
    relsig: 0,
    acqsig: 0,
    frsig: 0,
});

/// KD text/graphics mode of the active console.
static KD_MODE: AtomicU32 = AtomicU32::new(KD_TEXT);

/// Keyboard mode (K_XLATE=0 default; Xorg sets K_RAW=1 via KDSKBMODE).
static KB_MODE: AtomicU32 = AtomicU32::new(0);

/// VT switch lock (VT_LOCKSWITCH) — accepted, never consulted (single
/// owner protocol only).
static VT_LOCKED: AtomicBool = AtomicBool::new(false);

/// Waiters for VT_WAITACTIVE (woken on every ACTIVATE).
static SWITCH_WAITQ: WaitQueueHead = WaitQueueHead::new();

/// Is `devno` one of this layer's devices (major 4, minor 0..=NR_VT)?
pub fn is_vt_devno(devno: &crate::fs::dev_t::DevNo) -> bool {
    devno.major == crate::fs::dev_t::TTY_MAJOR && devno.minor <= NR_VT
}

/// open(): count the VT as in use (devfs_open calls this when the file's
/// device is a VT node).
pub fn vt_opened(devno: &crate::fs::dev_t::DevNo) {
    VT_REFS[devno.minor as usize].fetch_add(1, Ordering::Relaxed);
}

/// close(): release the in-use count.
pub fn vt_closed(devno: &crate::fs::dev_t::DevNo) {
    VT_REFS[devno.minor as usize].fetch_sub(1, Ordering::Relaxed);
}

/// The active VT number (sysfs /sys/class/tty/tty0/active content).
pub fn active_vt() -> u32 {
    ACTIVE_VT.load(Ordering::Acquire)
}

/// Wake every VT_WAITACTIVE sleeper (an ACTIVATE happened).
fn wake_switch_waiters() {
    SWITCH_WAITQ.wake_up_all();
}

/// One interruptible wait round for a VT switch. Returns Ok(()) when the
/// predicate already holds (caller re-checks), Err(EINTR) on a pending
/// signal. Same discipline as netlink_wait_round.
fn wait_switch_round() -> Result<(), i32> {
    let current = match crate::sched::current() {
        Some(t) => t,
        None => return Err(-11),
    };
    SWITCH_WAITQ.prepare_to_wait(current, false, true);
    if crate::signal::signal_pending() {
        SWITCH_WAITQ.finish_wait(current);
        crate::sched::dequeue_task(&*current);
        return Err(-4); // EINTR
    }
    crate::arch::cpu::restore_irq(true);
    crate::sched::schedule();
    SWITCH_WAITQ.finish_wait(current);
    if crate::signal::signal_pending() {
        return Err(-4);
    }
    Ok(())
}

// ============================================================================
// File operations (delegate to the shared console tty, like /dev/console)
// ============================================================================

fn vtdev_read(file: &crate::fs::file::File, buf: &mut [u8]) -> isize {
    crate::fs::devfs::condev_read(file, buf)
}

fn vtdev_write(file: &crate::fs::file::File, buf: &[u8]) -> isize {
    crate::fs::devfs::condev_write(file, buf)
}

fn vtdev_poll(file: &crate::fs::file::File, events: u16) -> u16 {
    crate::fs::devfs::condev_poll(file, events)
}

/// Last fd closed: release the VT in-use slot and free the stashed
/// DevNo (devfs_open boxed it; File::close runs exactly once per open
/// file description — the fput discipline in fs/file.rs).
// SAFETY: private_data holds a leaked Box<DevNo> installed by devfs_open.
fn vtdev_close(file: &crate::fs::file::File) -> i32 {
    // SAFETY: the Box was leaked for this File; close runs once.
    let ptr = match unsafe { *file.private_data.get() } {
        Some(p) => p,
        None => return 0,
    };
    let devno = unsafe { alloc::boxed::Box::from_raw(ptr as *mut crate::fs::dev_t::DevNo) };
    vt_closed(&devno);
    unsafe { *file.private_data.get() = None };
    0
}

/// VT device file operations.
pub static VTDEV_OPS: crate::fs::file::FileOps = crate::fs::file::FileOps::builder()
    .read(vtdev_read)
    .write(vtdev_write)
    .poll(vtdev_poll)
    .close(vtdev_close)
    .build();

// ============================================================================
// ioctl
// ============================================================================

/// VT/KD ioctl dispatch on the File's ops identity (evdev/pty pattern).
/// Returns None for non-VT files so sys_ioctl falls through.
pub fn vt_file_ioctl(file: &crate::fs::file::File, request: u32, arg: usize) -> Option<i64> {
    if !file
        .get_ops()
        .map_or(false, |ops| core::ptr::eq(ops as *const _, &VTDEV_OPS as *const _))
    {
        return None;
    }

    // The VT number this fd was opened against (devfs_open stashed the
    // DevNo; minor 0 = the tty0 master view, which addresses the ACTIVE
    // VT — exactly Linux's semantics for /dev/tty0).
    let vt = unsafe {
        match *(file.private_data.get()) {
            Some(p) => (*(p as *const crate::fs::dev_t::DevNo)).minor,
            None => 1,
        }
    };
    let _ = vt;

    use crate::errno::constants;
    let is_vt_cmd = matches!(request, VT_OPENQRY..=VT_UNLOCKSWITCH)
        || matches!(request, KIOCSOUND | KDGETLED | KDSETLED | KDGKBTYPE | KDSETMODE | KDGETMODE | KDGKBMODE | KDSKBMODE);
    if !is_vt_cmd {
        return None;
    }

    Some(match request {
        VT_OPENQRY => {
            // First VT with no open fd, skipping VT 1 when gdm holds it
            // (the caller opened tty1 precisely to mark it in use).
            let mut found: i32 = -1;
            for n in 1..=(NR_VT as usize) {
                if VT_REFS[n].load(Ordering::Relaxed) == 0 {
                    found = n as i32;
                    break;
                }
            }
            if found < 0 {
                return Some(-(constants::EBUSY as i64));
            }
            if arg == 0 || !crate::arch::uaccess::access_ok(arg, 4) {
                return Some(-(constants::EFAULT as i64));
            }
            // SAFETY: arg validated with access_ok(4) above.
            if !unsafe { crate::arch::uaccess::put_user(arg as *mut i32, found) } {
                return Some(-(constants::EFAULT as i64));
            }
            0
        }
        VT_GETMODE => {
            if arg == 0 || !crate::arch::uaccess::access_ok(arg, 8) {
                return Some(-(constants::EFAULT as i64));
            }
            let mut buf = [0u8; 8];
            {
                let m = VT_MODE.lock_irqsave();
                buf[0] = m.mode;
                buf[1] = m.waitv;
                buf[2..4].copy_from_slice(&m.relsig.to_le_bytes());
                buf[4..6].copy_from_slice(&m.acqsig.to_le_bytes());
                buf[6..8].copy_from_slice(&m.frsig.to_le_bytes());
            }
            // SAFETY: arg validated with access_ok(8) above.
            if unsafe { crate::arch::uaccess::copy_to_user(arg as *mut u8, buf.as_ptr(), 8) } > 0 {
                return Some(-(constants::EFAULT as i64));
            }
            0
        }
        VT_SETMODE => {
            if arg == 0 || !crate::arch::uaccess::access_ok(arg, 8) {
                return Some(-(constants::EFAULT as i64));
            }
            let mut buf = [0u8; 8];
            // SAFETY: arg validated with access_ok(8) above.
            if unsafe { crate::arch::uaccess::copy_from_user(buf.as_mut_ptr(), arg as *const u8, 8) }
                > 0
            {
                return Some(-(constants::EFAULT as i64));
            }
            let mut m = VT_MODE.lock_irqsave();
            m.mode = if buf[0] == VT_PROCESS { VT_PROCESS } else { VT_AUTO };
            m.waitv = buf[1];
            m.relsig = u16::from_le_bytes([buf[2], buf[3]]);
            m.acqsig = u16::from_le_bytes([buf[4], buf[5]]);
            m.frsig = u16::from_le_bytes([buf[6], buf[7]]);
            drop(m);
            0
        }
        VT_GETSTATE => {
            if arg == 0 || !crate::arch::uaccess::access_ok(arg, 6) {
                return Some(-(constants::EFAULT as i64));
            }
            let active = ACTIVE_VT.load(Ordering::Acquire);
            // Allocated-console bitmask: bit n-1 set for VT n (all are
            // statically allocated here).
            let mut mask: u16 = 0;
            for n in 1..=NR_VT {
                mask |= 1 << (n - 1);
            }
            // struct vt_stat fields are unsigned short (u16) — v_active
            // must be narrowed before to_le_bytes or copy_from_slice
            // panics (gdmr31: the worker's jump_to_vt VT_GETSTATE hit
            // "source slice length (4) does not match destination slice
            // length (2)" and took the box down).
            let active16 = active as u16;
            let mut buf = [0u8; 6];
            buf[0..2].copy_from_slice(&active16.to_le_bytes());
            buf[2..4].copy_from_slice(&0u16.to_le_bytes()); // v_signal
            buf[4..6].copy_from_slice(&mask.to_le_bytes());
            // SAFETY: arg validated with access_ok(6) above.
            if unsafe { crate::arch::uaccess::copy_to_user(arg as *mut u8, buf.as_ptr(), 6) } > 0 {
                return Some(-(constants::EFAULT as i64));
            }
            0
        }
        VT_SENDSIG | VT_RELDISP => {
            // No competing VT_PROCESS owner exists to handshake with;
            // acknowledge (the arg is VT_ACKACQ/1 and is ignored).
            0
        }
        VT_ACTIVATE => {
            let n = arg as u32;
            if n == 0 || n > NR_VT {
                return Some(-(constants::EINVAL as i64));
            }
            ACTIVE_VT.store(n, Ordering::Release);
            wake_switch_waiters();
            0
        }
        VT_WAITACTIVE => {
            let n = arg as u32;
            if n == 0 || n > NR_VT {
                return Some(-(constants::EINVAL as i64));
            }
            loop {
                if ACTIVE_VT.load(Ordering::Acquire) == n {
                    return Some(0);
                }
                match wait_switch_round() {
                    Ok(()) => continue,
                    Err(e) => return Some(e as i64),
                }
            }
        }
        VT_DISALLOCATE => {
            // Static VT set: nothing to free. A VT that is open stays.
            0
        }
        VT_RESIZE | VT_RESIZEX => {
            // struct winsz/conssize from user — accept the size report
            // without splitting it per-VT (single console).
            0
        }
        VT_LOCKSWITCH => {
            VT_LOCKED.store(true, Ordering::Release);
            0
        }
        VT_UNLOCKSWITCH => {
            VT_LOCKED.store(false, Ordering::Release);
            0
        }
        KIOCSOUND => 0, // no PC speaker
        KDGETLED => {
            if arg == 0 || !crate::arch::uaccess::access_ok(arg, 1) {
                return Some(-(constants::EFAULT as i64));
            }
            // SAFETY: arg validated with access_ok(1) above.
            if !unsafe { crate::arch::uaccess::put_user(arg as *mut u8, 0u8) } {
                return Some(-(constants::EFAULT as i64));
            }
            0
        }
        KDSETLED => 0,
        KDGKBTYPE => {
            if arg == 0 || !crate::arch::uaccess::access_ok(arg, 1) {
                return Some(-(constants::EFAULT as i64));
            }
            // SAFETY: arg validated with access_ok(1) above.
            if !unsafe { crate::arch::uaccess::put_user(arg as *mut u8, KB_101) } {
                return Some(-(constants::EFAULT as i64));
            }
            0
        }
        KDSETMODE => {
            if arg as u32 > KD_GRAPHICS {
                return Some(-(constants::EINVAL as i64));
            }
            KD_MODE.store(arg as u32, Ordering::Release);
            0
        }
        KDGETMODE => {
            if arg == 0 || !crate::arch::uaccess::access_ok(arg, 4) {
                return Some(-(constants::EFAULT as i64));
            }
            // SAFETY: arg validated with access_ok(4) above.
            if !unsafe { crate::arch::uaccess::put_user(arg as *mut i32, KD_MODE.load(Ordering::Acquire) as i32) } {
                return Some(-(constants::EFAULT as i64));
            }
            0
        }
        KDGKBMODE => {
            if arg == 0 || !crate::arch::uaccess::access_ok(arg, 4) {
                return Some(-(constants::EFAULT as i64));
            }
            // SAFETY: arg validated with access_ok(4) above.
            if !unsafe { crate::arch::uaccess::put_user(arg as *mut i32, KB_MODE.load(Ordering::Acquire) as i32) } {
                return Some(-(constants::EFAULT as i64));
            }
            0
        }
        KDSKBMODE => {
            // K_XLATE=0 / K_RAW=1 / K_MEDIUMRAW=2 / K_UNICODE=4 — store
            // the scancode-vs-translation choice (no keymap engine yet).
            KB_MODE.store(arg as u32, Ordering::Release);
            0
        }
        _ => return Some(-(constants::ENOTTY as i64)),
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test_case]
    fn test_vt_openqry_skips_in_use() {
        // Reset refs for the test's VTs.
        VT_REFS[1].store(1, Ordering::Relaxed); // gdm holds tty1
        VT_REFS[2].store(0, Ordering::Relaxed);
        // Simulate the OPENQRY scan.
        let mut found = 0;
        for n in 1..=(NR_VT as usize) {
            if VT_REFS[n].load(Ordering::Relaxed) == 0 {
                found = n;
                break;
            }
        }
        crate::test_assert_eq!(found, 2, "openqry returns first free VT");
        VT_REFS[1].store(0, Ordering::Relaxed);
    }

    #[test_case]
    fn test_vt_mode_roundtrip_layout() {
        let mut buf = [0u8; 8];
        {
            let mut m = VT_MODE.lock_irqsave();
            m.mode = VT_PROCESS;
            m.relsig = 10; // SIGUSR1
            m.acqsig = 10;
            buf[0] = m.mode;
            buf[1] = m.waitv;
            buf[2..4].copy_from_slice(&m.relsig.to_le_bytes());
            buf[4..6].copy_from_slice(&m.acqsig.to_le_bytes());
            buf[6..8].copy_from_slice(&m.frsig.to_le_bytes());
            m.mode = VT_AUTO;
            m.relsig = 0;
            m.acqsig = 0;
        }
        // struct vt_mode: mode@0, waitv@1, relsig@2, acqsig@4, frsig@6.
        crate::test_assert_eq!(buf[0], VT_PROCESS);
        crate::test_assert_eq!(u16::from_le_bytes([buf[2], buf[3]]), 10);
        crate::test_assert_eq!(u16::from_le_bytes([buf[4], buf[5]]), 10);
    }
}
