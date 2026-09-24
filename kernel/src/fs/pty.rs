//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! PTY (pseudo-terminal) pairs — Linux-semantics simplified
//!
//! Data flow:
//! - master write → slave's TtyDevice line discipline (canonical editing,
//!   echo, ISIG) → slave input queue; echo lands in the master read buffer
//! - slave read  → own input queue (line-discipline processed)
//! - slave write → (OPOST/ONLCR) → master read buffer
//! - master read → master read buffer (program output + echo)
//!
//! Lifecycle:
//! - open("/dev/ptmx") allocates a pair: master fd + /dev/pts/N slave node
//! - close(last master) → slave reads EOF, slave writers get EPIPE/SIGPIPE,
//!   slave fg pgrp gets SIGHUP
//! - close(last slave) → master read/poll get EIO/POLLHUP
//! - pair (and its /dev/pts/N node) is destroyed when both sides are closed
//!
//! Syscall surface (musl):
//! - posix_openpt(flags) = open("/dev/ptmx", flags) — no dedicated syscall
//! - grantpt/unlockpt = no-ops in the kernel (TIOCSPTLCK returns 0)
//! - ptsname_r uses ioctl(TIOCGPTN) on the master + open("/dev/pts/N")

use alloc::collections::BTreeMap;
use alloc::format;
use alloc::sync::Arc;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

use crate::fs::dev_t::DevNo;
use crate::fs::file::{File, FileOps, FileFlags};
use crate::fs::tty::{
    termios_from_termio_bytes, termios_from_user_bytes, termios_to_termio_bytes,
    termios_to_user_bytes, TtyDevice, TERMIOS_USER_SIZE, TERMIO_USER_SIZE,
};
use crate::process::wait::WaitQueueHead;
use crate::signal::Signal;
use crate::sync::spinlock::Spinlock;

// ============================================================================
// Device numbers / ioctl numbers
// ============================================================================

/// Linux TTYAUX major; /dev/ptmx is minor 2.
pub const TTYAUX_MAJOR: u32 = 5;
pub const PTMX_MINOR: u32 = 2;
/// /dev/ptmx device number.
pub const DEV_PTMX: DevNo = DevNo::new(TTYAUX_MAJOR, PTMX_MINOR);
/// UNIX98 pty slaves use major 136, minor = pty index.
pub const PTY_SLAVE_MAJOR: u32 = 136;

// ioctl request numbers (asm-generic ABI)
const TCGETS: u32 = 0x5401;
const TCSETS: u32 = 0x5402;
const TCSETSW: u32 = 0x5403;
const TCSETSF: u32 = 0x5404;
const TCGETA: u32 = 0x5405;
const TCSETA: u32 = 0x5406;
const TCSETAW: u32 = 0x5407;
const TCSETAF: u32 = 0x5408;
const TIOCGPGRP: u32 = 0x540F;
const TIOCSPGRP: u32 = 0x5410;
const TIOCGWINSZ: u32 = 0x5413;
const TIOCSWINSZ: u32 = 0x5414;
const FIONREAD: u32 = 0x541B;
/// TIOCGPTN — _IOR('T', 0x30, unsigned int): pty index (ptsname_r)
const TIOCGPTN: u32 = 0x8004_5440;
/// TIOCSPTLCK — _IOW('T', 0x31, int): unlock (no-op, always unlocked)
const TIOCSPTLCK: u32 = 0x4004_5431;

// ============================================================================
// Master read buffer (slave output + echo, waiting for master read)
// ============================================================================

/// Master-side buffer capacity (Linux uses one 8K buffer per pty).
const MASTER_RX_BUF: usize = 8192;

struct MasterRx {
    data: [u8; MASTER_RX_BUF],
    /// read position (master read side)
    head: usize,
    /// write position (slave write / echo side)
    tail: usize,
}

impl MasterRx {
    const fn new() -> Self {
        Self {
            data: [0; MASTER_RX_BUF],
            head: 0,
            tail: 0,
        }
    }

    fn len(&self) -> usize {
        self.tail - self.head
    }

    fn space(&self) -> usize {
        MASTER_RX_BUF - self.len()
    }

    /// Append ALL bytes or nothing (atomic for the ONLCR two-byte pair).
    fn write_exact(&mut self, bytes: &[u8]) -> bool {
        if self.space() < bytes.len() {
            return false;
        }
        for &b in bytes {
            self.data[self.tail % MASTER_RX_BUF] = b;
            self.tail += 1;
        }
        true
    }

    /// Copy out up to buf.len() buffered bytes.
    fn read(&mut self, buf: &mut [u8]) -> usize {
        let n = self.len().min(buf.len());
        for i in 0..n {
            buf[i] = self.data[(self.head + i) % MASTER_RX_BUF];
        }
        self.head += n;
        n
    }
}

// ============================================================================
// PtyPair
// ============================================================================

pub struct PtyPair {
    /// pty index N → /dev/pts/N (monotonic, never reused)
    pub index: u32,
    /// slave-side terminal: line discipline, termios, input queue
    pub tty: TtyDevice,
    /// slave output + echo waiting for master read
    master_rx: Spinlock<MasterRx>,
    /// master readers blocked on empty master_rx
    master_waitq: WaitQueueHead,
    /// slave writers blocked on full master_rx
    master_space_waitq: WaitQueueHead,
    /// open-file-description refcounts (one per open, not per dup)
    master_refs: AtomicI32,
    slave_refs: AtomicI32,
    /// last master description closed
    master_closed: AtomicBool,
    /// last slave description closed
    slave_closed: AtomicBool,
}

impl PtyPair {
    fn new(index: u32) -> Self {
        Self {
            index,
            tty: TtyDevice::new(pty_output, 0),
            master_rx: Spinlock::new(MasterRx::new()),
            master_waitq: WaitQueueHead::new(),
            master_space_waitq: WaitQueueHead::new(),
            master_refs: AtomicI32::new(0),
            slave_refs: AtomicI32::new(0),
            master_closed: AtomicBool::new(false),
            slave_closed: AtomicBool::new(false),
        }
    }

    fn master_rx_len(&self) -> usize {
        self.master_rx.lock_irqsave().len()
    }

    fn master_rx_space(&self) -> usize {
        self.master_rx.lock_irqsave().space()
    }

    fn master_rx_read(&self, buf: &mut [u8]) -> usize {
        self.master_rx.lock_irqsave().read(buf)
    }

    fn master_rx_write_exact(&self, bytes: &[u8]) -> bool {
        self.master_rx.lock_irqsave().write_exact(bytes)
    }
}

/// TtyDevice output sink for a pty slave: program output / echo lands in
/// the master read buffer. `ctx` is the owning PtyPair (set before the pair
/// is published; valid whenever the TtyDevice is reachable).
fn pty_output(ctx: usize, bytes: &[u8]) {
    if ctx == 0 {
        return;
    }
    // SAFETY: ctx is the Arc::as_ptr of the owning PtyPair, set once at
    // alloc time; the pair outlives every File referencing it.
    let pair = unsafe { &*(ctx as *const PtyPair) };
    let _ = pair.master_rx_write_exact(bytes);
}

// ============================================================================
// Global pty table
// ============================================================================

/// Live pty pairs by index.
static PTY_TABLE: Spinlock<BTreeMap<u32, Arc<PtyPair>>> = Spinlock::new(BTreeMap::new());
/// Monotonic pty index allocator (indexes are never reused).
static NEXT_PTY_INDEX: AtomicU32 = AtomicU32::new(0);

/// Allocate a new pty pair: registers the /dev/pts/N slave node.
pub fn alloc_pty() -> Option<Arc<PtyPair>> {
    let index = NEXT_PTY_INDEX.fetch_add(1, Ordering::Relaxed);
    // Leave headroom: minor numbers are u32; 0xFFF_FFFF is effectively
    // "never reached" for a kernel session.
    if index >= 0xFFF_FFFF {
        NEXT_PTY_INDEX.store(0xFFF_FFFF, Ordering::Relaxed);
        return None;
    }

    let pair = Arc::new(PtyPair::new(index));
    // Back-pointer for the tty output sink (before publication).
    pair.tty.set_output_ctx(Arc::as_ptr(&pair) as usize);

    // Register the slave device ops so devfs get_file_ops can resolve
    // /dev/pts/N → PTY_SLAVE_OPS.
    crate::fs::devfs::registry::register_char_device(
        DevNo::new(PTY_SLAVE_MAJOR, index),
        &PTY_SLAVE_OPS,
    )
    .ok()?;

    // Create the dynamic /dev/pts/N node (evicts any stale negative dentry).
    if !devfs_add_slave_node(index) {
        crate::fs::devfs::registry::unregister_char_device(DevNo::new(PTY_SLAVE_MAJOR, index));
        return None;
    }

    PTY_TABLE.lock_irqsave().insert(index, pair.clone());
    Some(pair)
}

/// Destroy a pair: drop it from the table, unregister the devno, remove the
/// /dev/pts/N node. Idempotent (concurrent last-master/last-slave closes).
fn destroy_pair(pair: &PtyPair) {
    let removed = PTY_TABLE.lock_irqsave().remove(&pair.index);
    if removed.is_none() {
        return; // someone else already destroyed it
    }
    crate::fs::devfs::registry::unregister_char_device(DevNo::new(PTY_SLAVE_MAJOR, pair.index));
    devfs_remove_slave_node(pair.index);
}

// ============================================================================
// devfs integration
// ============================================================================

/// Called from devfs::init() — register the ptmx device ops.
/// (The /dev/ptmx node itself and the /dev/pts directory are created by
/// devfs::init() alongside this.)
pub fn devfs_register() {
    let _ = crate::fs::devfs::registry::register_char_device(DEV_PTMX, &PTMX_OPS);
}

/// Create the dynamic /dev/pts/N char node and evict any stale dentry.
/// NOTE: devfs paths are relative to the devfs root (mounted at /dev) —
/// "pts/N", not "/dev/pts/N".
fn devfs_add_slave_node(index: u32) -> bool {
    let path = format!("pts/{}", index);
    let mode = 0o666 | 0o020000; // S_IFCHR | rw-rw-rw-
    if crate::fs::devfs::mknod(&path, DevNo::new(PTY_SLAVE_MAJOR, index), mode).is_err() {
        return false;
    }
    evict_pts_dentry(index);
    true
}

/// Remove the dynamic /dev/pts/N node and evict its cached dentry.
fn devfs_remove_slave_node(index: u32) {
    let path = format!("pts/{}", index);
    let _ = crate::fs::devfs::remove_node(&path);
    evict_pts_dentry(index);
}

/// Evict any cached (negative OR positive) dentry named "N" under /dev/pts:
/// a negative entry would mask the freshly created node, and a positive
/// entry would outlive the removed one.
fn evict_pts_dentry(index: u32) {
    if let Ok(vpath) = crate::fs::vfs::path_lookup("/dev/pts", 0) {
        if let Some(pts_dentry) = vpath.dentry {
            pts_dentry.remove_child(&format!("{}", index));
        }
    }
}

// ============================================================================
// open hooks (called from the devfs inode open callback)
// ============================================================================

/// open("/dev/ptmx") — allocate a fresh pty pair for this description.
pub fn ptmx_open(file: &File) -> i32 {
    let pair = match alloc_pty() {
        Some(p) => p,
        None => return crate::errno::Errno::OutOfMemory.as_neg_i32(),
    };
    pair.master_refs.fetch_add(1, Ordering::AcqRel);
    // Ownership of one Arc ref moves into the File's private_data; the
    // close op reconstructs it with Arc::from_raw.
    file.set_private_data(Arc::into_raw(pair) as *mut u8);
    0
}

/// open("/dev/pts/N") — attach this description to the live pair N.
pub fn slave_open(file: &File, minor: u32) -> i32 {
    let pair = PTY_TABLE.lock_irqsave().get(&minor).cloned();
    let pair = match pair {
        Some(p) => p,
        None => return crate::errno::Errno::NoSuchFileOrDirectory.as_neg_i32(),
    };
    if pair.master_closed.load(Ordering::Acquire) {
        return crate::errno::Errno::IOError.as_neg_i32();
    }
    // Reopen after all previous slaves closed: clear the stale closed flag
    // (0→1 transition), or the master would keep seeing EIO/POLLHUP.
    if pair.slave_refs.fetch_add(1, Ordering::AcqRel) == 0 {
        pair.slave_closed.store(false, Ordering::Release);
    }
    file.set_private_data(Arc::into_raw(pair) as *mut u8);
    0
}

// ============================================================================
// File helpers
// ============================================================================

/// Borrow the PtyPair behind a master/slave File (None after close).
fn pair_of(file: &File) -> Option<&'static PtyPair> {
    // SAFETY: private_data is either null or an Arc::into_raw(PtyPair)
    // installed by ptmx_open/slave_open; the Arc ref held by the File keeps
    // it alive.
    let ptr = unsafe { *file.private_data.get() }? as *const PtyPair;
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { &*ptr })
}

fn nonblock(file: &File) -> bool {
    file.flags().bits() & FileFlags::O_NONBLOCK != 0
}

fn send_sigpipe() {
    if let Some(current) = crate::sched::current() {
        let _ = crate::signal::send_signal((*current).pid(), Signal::SIGPIPE as i32);
    }
}

// ============================================================================
// Master file operations (/dev/ptmx)
// ============================================================================

fn ptmx_read(file: &File, buf: &mut [u8]) -> isize {
    use crate::errno::constants::{EAGAIN, EBADF, EINTR, EIO};

    let pair = match pair_of(file) {
        Some(p) => p,
        None => return -(EBADF as isize),
    };
    if buf.is_empty() {
        return 0;
    }
    let nb = nonblock(file);

    loop {
        // Slave side gone: master read fails with EIO (Linux semantics).
        if pair.slave_closed.load(Ordering::Acquire) {
            return -(EIO as isize);
        }
        let n = pair.master_rx_read(buf);
        if n > 0 {
            // Space freed for slave writers.
            pair.master_space_waitq.wake_up_all();
            return n as isize;
        }
        if nb {
            return -(EAGAIN as isize);
        }

        // Blocking read (pipe.rs wait discipline).
        let current = match crate::sched::current() {
            Some(task) => task,
            None => return 0,
        };

        pair.master_waitq.prepare_to_wait(current, false, true);

        // Re-check AFTER registering (lost-wakeup guard).
        if pair.master_rx_len() > 0 || pair.slave_closed.load(Ordering::Acquire) {
            pair.master_waitq.finish_wait(current);
            crate::sched::dequeue_task(&*current);
            continue;
        }

        if crate::signal::signal_pending() {
            pair.master_waitq.finish_wait(current);
            crate::sched::dequeue_task(&*current);
            return -(EINTR as isize);
        }

        // R54: re-arm interrupts so ticks/IPIs reach this CPU.
        crate::arch::riscv64::cpu::restore_irq(true);
        crate::sched::schedule();

        pair.master_waitq.finish_wait(current);

        if crate::signal::signal_pending() {
            return -(EINTR as isize);
        }
    }
}

fn ptmx_write(file: &File, buf: &[u8]) -> isize {
    use crate::errno::constants::{EBADF, EIO};

    let pair = match pair_of(file) {
        Some(p) => p,
        None => return -(EBADF as isize),
    };
    // Writing to a master whose slave is gone: EIO.
    if pair.slave_closed.load(Ordering::Acquire) {
        return -(EIO as isize);
    }
    // Keyboard input: run each byte through the slave line discipline
    // (canonical editing / echo / ISIG). Never blocks.
    for &c in buf {
        pair.tty.receive_byte(c);
    }
    buf.len() as isize
}

fn ptmx_poll(file: &File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;

    let pair = match pair_of(file) {
        Some(p) => p,
        None => return 0,
    };
    let mut ready = 0u16;
    if events & POLLIN != 0 && pair.master_rx_len() > 0 {
        ready |= POLLIN | POLLRDNORM;
    }
    if events & POLLOUT != 0 && !pair.slave_closed.load(Ordering::Acquire) {
        ready |= POLLOUT | POLLWRNORM;
    }
    // Slave gone: POLLHUP is reported regardless of requested events.
    if pair.slave_closed.load(Ordering::Acquire) {
        ready |= POLLHUP;
    }
    ready
}

fn ptmx_close(file: &File) -> i32 {
    // SAFETY: private_data.get() reads our own Arc::into_raw pointer.
    if let Some(ptr) = unsafe { file.private_data.get().replace(None) } {
        // Reconstruct the Arc installed by ptmx_open, dropping one ref.
        let pair = unsafe { Arc::from_raw(ptr as *const PtyPair) };

        if pair.master_refs.fetch_sub(1, Ordering::AcqRel) == 1 {
            // Last master description closed.
            pair.master_closed.store(true, Ordering::Release);
            // Slave readers get persistent EOF.
            pair.tty.hungup.store(true, Ordering::Release);
            pair.tty.read_waitq.wake_up_all();
            // Slave writers blocked on a full master buffer: release them
            // into the EPIPE path.
            pair.master_space_waitq.wake_up_all();
            // SIGHUP the slave's foreground process group.
            let pgid = pair.tty.fg_pgrp.load(Ordering::Acquire);
            if pgid != 0 {
                crate::signal::send_signal_to_pgid(pgid, Signal::SIGHUP as i32);
            }
            // Destroy the pair if the slave side is gone too.
            if pair.slave_refs.load(Ordering::Acquire) == 0 {
                destroy_pair(&pair);
            }
        }
        drop(pair);
    }
    0
}

/// /dev/ptmx file operations (every open description allocates its own pty;
/// per-pair state lives in private_data).
pub static PTMX_OPS: FileOps = FileOps {
    read: Some(ptmx_read),
    write: Some(ptmx_write),
    lseek: None,
    close: Some(ptmx_close),
    poll: Some(ptmx_poll),
};

// ============================================================================
// Slave file operations (/dev/pts/N)
// ============================================================================

fn pty_slave_read(file: &File, buf: &mut [u8]) -> isize {
    use crate::errno::constants::EBADF;

    let pair = match pair_of(file) {
        Some(p) => p,
        None => return -(EBADF as isize),
    };
    // Master gone → read_input returns EOF (hungup) — task spec.
    pair.tty.read_input(buf, nonblock(file))
}

fn pty_slave_write(file: &File, buf: &[u8]) -> isize {
    use crate::errno::constants::{EAGAIN, EBADF, EINTR, EPIPE};

    let pair = match pair_of(file) {
        Some(p) => p,
        None => return -(EBADF as isize),
    };
    if pair.master_closed.load(Ordering::Acquire) {
        send_sigpipe();
        return -(EPIPE as isize);
    }
    if buf.is_empty() {
        return 0;
    }
    let nb = nonblock(file);

    // Program output path: OPOST/ONLCR translation then master buffer.
    let onlcr = pair.tty.output_translates_nl();

    let mut written: usize = 0;
    let mut i: usize = 0;
    while i < buf.len() {
        let out: &[u8] = if onlcr && buf[i] == b'\n' {
            b"\r\n"
        } else {
            core::slice::from_ref(&buf[i])
        };

        if pair.master_rx_write_exact(out) {
            pair.master_waitq.wake_up_all();
            written += 1;
            i += 1;
            continue;
        }

        // Master buffer full.
        if pair.master_closed.load(Ordering::Acquire) {
            send_sigpipe();
            return if written > 0 { written as isize } else { -(EPIPE as isize) };
        }
        if nb {
            return if written > 0 { written as isize } else { -(EAGAIN as isize) };
        }

        // Block for space (pipe.rs wait discipline).
        let current = match crate::sched::current() {
            Some(task) => task,
            None => return written as isize,
        };

        pair.master_space_waitq.prepare_to_wait(current, false, true);

        if pair.master_rx_space() >= out.len()
            || pair.master_closed.load(Ordering::Acquire)
        {
            pair.master_space_waitq.finish_wait(current);
            crate::sched::dequeue_task(&*current);
            continue;
        }

        if crate::signal::signal_pending() {
            pair.master_space_waitq.finish_wait(current);
            crate::sched::dequeue_task(&*current);
            if written > 0 {
                return written as isize;
            }
            return -(EINTR as isize);
        }

        // R54: re-arm interrupts so ticks/IPIs reach this CPU.
        crate::arch::riscv64::cpu::restore_irq(true);
        crate::sched::schedule();

        pair.master_space_waitq.finish_wait(current);

        if crate::signal::signal_pending() {
            if written > 0 {
                return written as isize;
            }
            return -(EINTR as isize);
        }
    }

    written as isize
}

fn pty_slave_poll(file: &File, events: u16) -> u16 {
    use crate::syscall::misc::poll_events::*;

    let pair = match pair_of(file) {
        Some(p) => p,
        None => return 0,
    };
    let mut ready = 0u16;
    if events & POLLIN != 0
        && (pair.tty.input_poll_ready() || pair.tty.hungup.load(Ordering::Acquire))
    {
        ready |= POLLIN | POLLRDNORM;
    }
    if events & POLLOUT != 0 && pair.master_rx_space() > 0 {
        ready |= POLLOUT | POLLWRNORM;
    }
    // Master gone: POLLHUP regardless of requested events.
    if pair.tty.hungup.load(Ordering::Acquire) {
        ready |= POLLHUP;
    }
    ready
}

fn pty_slave_close(file: &File) -> i32 {
    // SAFETY: private_data.get() reads our own Arc::into_raw pointer.
    if let Some(ptr) = unsafe { file.private_data.get().replace(None) } {
        // Reconstruct the Arc installed by slave_open, dropping one ref.
        let pair = unsafe { Arc::from_raw(ptr as *const PtyPair) };

        if pair.slave_refs.fetch_sub(1, Ordering::AcqRel) == 1 {
            // Last slave description closed.
            pair.slave_closed.store(true, Ordering::Release);
            // Master read/poll get EIO/POLLHUP.
            pair.master_waitq.wake_up_all();
            // Destroy the pair if the master side is gone too.
            if pair.master_refs.load(Ordering::Acquire) == 0 {
                destroy_pair(&pair);
            }
        }
        drop(pair);
    }
    0
}

/// /dev/pts/N file operations (shared table; per-pair state in private_data).
pub static PTY_SLAVE_OPS: FileOps = FileOps {
    read: Some(pty_slave_read),
    write: Some(pty_slave_write),
    lseek: None,
    close: Some(pty_slave_close),
    poll: Some(pty_slave_poll),
};

// ============================================================================
// Per-fd tty ioctl dispatch
// ============================================================================

/// Handle tty ioctls for pty master/slave Files.
///
/// Returns Some(ret) when `file` is a pty end and the request was handled
/// (or is a pty-specific stub); returns None when the file is not a pty or
/// the request must fall through to the generic handler (FIONBIO, ...).
pub fn pty_ioctl(file: &File, request: u32, arg: usize) -> Option<i64> {
    use crate::arch::riscv64::uaccess::{access_ok, copy_from_user, copy_to_user};
    use crate::errno::constants::EFAULT;

    let ops = file.get_ops()?;
    let is_master = core::ptr::eq(ops as *const _, &PTMX_OPS as *const _);
    let is_slave = core::ptr::eq(ops as *const _, &PTY_SLAVE_OPS as *const _);
    if !is_master && !is_slave {
        return None; // not a pty — generic path
    }
    let pair = match pair_of(file) {
        Some(p) => p,
        None => return Some(-(crate::errno::constants::EBADF as i64)),
    };

    // SAFETY: user pointers are access_ok-validated before each copy; kernel
    // buffers are stack arrays of the exact request size.
    unsafe {
        match request {
            TCGETS => {
                if arg == 0 || !access_ok(arg, TERMIOS_USER_SIZE) {
                    return Some(-(EFAULT as i64));
                }
                let tio = pair.tty.get_termios();
                let mut kbuf = [0u8; TERMIOS_USER_SIZE];
                termios_to_user_bytes(&tio, &mut kbuf);
                if copy_to_user(arg as *mut u8, kbuf.as_ptr(), TERMIOS_USER_SIZE) > 0 {
                    return Some(-(EFAULT as i64));
                }
                Some(0)
            }
            TCSETS | TCSETSW | TCSETSF => {
                if arg == 0 || !access_ok(arg, TERMIOS_USER_SIZE) {
                    return Some(-(EFAULT as i64));
                }
                let mut kbuf = [0u8; TERMIOS_USER_SIZE];
                if copy_from_user(kbuf.as_mut_ptr(), arg as *const u8, TERMIOS_USER_SIZE) > 0 {
                    return Some(-(EFAULT as i64));
                }
                if request == TCSETSF {
                    pair.tty.flush_input();
                }
                pair.tty.set_termios(termios_from_user_bytes(&kbuf));
                Some(0)
            }
            TCGETA => {
                if arg == 0 || !access_ok(arg, TERMIO_USER_SIZE) {
                    return Some(-(EFAULT as i64));
                }
                let tio = pair.tty.get_termios();
                let mut kbuf = [0u8; TERMIO_USER_SIZE];
                termios_to_termio_bytes(&tio, &mut kbuf);
                if copy_to_user(arg as *mut u8, kbuf.as_ptr(), TERMIO_USER_SIZE) > 0 {
                    return Some(-(EFAULT as i64));
                }
                Some(0)
            }
            TCSETA | TCSETAW | TCSETAF => {
                if arg == 0 || !access_ok(arg, TERMIO_USER_SIZE) {
                    return Some(-(EFAULT as i64));
                }
                let mut kbuf = [0u8; TERMIO_USER_SIZE];
                if copy_from_user(kbuf.as_mut_ptr(), arg as *const u8, TERMIO_USER_SIZE) > 0 {
                    return Some(-(EFAULT as i64));
                }
                let base = pair.tty.get_termios();
                let tio = termios_from_termio_bytes(&kbuf, &base);
                if request == TCSETAF {
                    pair.tty.flush_input();
                }
                pair.tty.set_termios(tio);
                Some(0)
            }
            TIOCGWINSZ => {
                if arg == 0 || !access_ok(arg, 8) {
                    return Some(-(EFAULT as i64));
                }
                let ws = pair.tty.get_winsize().to_le_bytes();
                if copy_to_user(arg as *mut u8, ws.as_ptr(), 8) > 0 {
                    return Some(-(EFAULT as i64));
                }
                Some(0)
            }
            TIOCSWINSZ => {
                if arg == 0 || !access_ok(arg, 8) {
                    return Some(-(EFAULT as i64));
                }
                let mut kbuf = [0u8; 8];
                if copy_from_user(kbuf.as_mut_ptr(), arg as *const u8, 8) > 0 {
                    return Some(-(EFAULT as i64));
                }
                let ws = crate::fs::tty::WinSize::from_le_bytes(&kbuf);
                if pair.tty.set_winsize(ws) {
                    // Notify the foreground process group, like Linux.
                    pair.tty.send_sigwinch();
                }
                Some(0)
            }
            TIOCGPGRP => {
                if arg == 0 || !access_ok(arg, 4) {
                    return Some(-(EFAULT as i64));
                }
                let pgid = pair.tty.fg_pgrp.load(Ordering::Acquire);
                if copy_to_user(arg as *mut u8, pgid.to_le_bytes().as_ptr(), 4) > 0 {
                    return Some(-(EFAULT as i64));
                }
                Some(0)
            }
            TIOCSPGRP => {
                if arg == 0 || !access_ok(arg, 4) {
                    return Some(-(EFAULT as i64));
                }
                let mut kbuf = [0u8; 4];
                if copy_from_user(kbuf.as_mut_ptr(), arg as *const u8, 4) > 0 {
                    return Some(-(EFAULT as i64));
                }
                let pgid = u32::from_le_bytes(kbuf);
                pair.tty.fg_pgrp.store(pgid, Ordering::Release);
                Some(0)
            }
            TIOCGPTN if is_master => {
                if arg == 0 || !access_ok(arg, 4) {
                    return Some(-(EFAULT as i64));
                }
                if copy_to_user(arg as *mut u8, pair.index.to_le_bytes().as_ptr(), 4) > 0 {
                    return Some(-(EFAULT as i64));
                }
                Some(0)
            }
            // unlockpt: kernel-side ptys are never locked — report unlocked.
            TIOCSPTLCK => {
                if arg != 0 && !access_ok(arg, 4) {
                    return Some(-(EFAULT as i64));
                }
                Some(0)
            }
            FIONREAD => {
                if arg == 0 || !access_ok(arg, 4) {
                    return Some(-(EFAULT as i64));
                }
                let count: u32 = if is_master {
                    pair.master_rx_len() as u32
                } else {
                    pair.tty.fionread_count() as u32
                };
                if copy_to_user(arg as *mut u8, count.to_le_bytes().as_ptr(), 4) > 0 {
                    return Some(-(EFAULT as i64));
                }
                Some(0)
            }
            _ => None, // FIONBIO and unknown requests → generic path
        }
    }
}
