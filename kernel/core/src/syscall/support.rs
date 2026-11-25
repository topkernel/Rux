//! Framework-side syscall SUPPORT surface (crate-flip State B): the
//! lib-consumed plumbing that historically lived in the service files.
//! The epoll/timerfd/eventfd internals STAY in the service crate (their
//! types live there); the lib reaches them through this HOOK TABLE,
//! registered by syscall::register_all — the same data-seam shape
//! as the dispatch tables (nullable fns no-op until boot registration;
//! the diagnostics are trace-gated anyway). The small PURE items
//! (RW_CHUNK, clamp_rw_count, the mmap ring) moved here bodily.

#![forbid(unsafe_code)]
use crate::fs::File;

/// The per-iteration chunk for bulk read/write staging (the io
/// service's historical constant — io_uring references it).
pub const RW_CHUNK: usize = 64 * 1024;

/// Clamp a user-supplied transfer length to MAX_RW_COUNT.
#[inline]
pub fn clamp_rw_count(count: usize) -> usize {
    if count > crate::arch::uaccess::MAX_RW_COUNT {
        crate::arch::uaccess::MAX_RW_COUNT
    } else {
        count
    }
}

// ==================== the debug/event hook table ====================

/// The lib-consumed service hooks. Every entry defaults to a no-op (or
/// false/None); syscall::register_all publishes the real
/// implementations at boot — before the framework impl runs.
#[derive(Clone, Copy)]
pub struct SyscallHooks {
    /// rux:socksnap: dump every epoll instance's interest set.
    pub epoll_interest_snapshot: fn(),
    /// taskdump: the timerfd wheel census.
    pub timerfd_wheel_audit: fn(),
    /// taskdump: pid1's epoll interest (the launcher-watch face).
    pub pid1_epoll_interest_dump: fn(crate::process::task::Pid),
    /// /proc/sys/kernel/random + uuid sources.
    pub kernel_random_bytes: fn(&mut [u8]),
    /// futex's eventfd readiness probe (oh67 hunt).
    pub oh67_fd_ready: fn(&File) -> Option<alloc::string::String>,
    /// futex's absolute-deadline helpers.
    pub jiffies_covering_deadline: fn(u64) -> u64,
    pub deadline_ns_to_hres_ticks: fn(u64) -> u64,
    /// the timer wheel's timerfd expiry notify.
    pub timerfd_expire_notify: fn(u64),
    /// the x86 trap stall-dump (param hunt).
    pub param_stall_dump: fn(),
    /// eventfd ops identity (futex/io_uring compares).
    pub is_eventfd_ops: fn(&'static crate::fs::FileOps) -> bool,
    /// EFAULT census line (the sleep-EFAULT hunt's per-call trace).
    pub syscall_eio_line: fn(u64, &[u64; 6]),
    /// net/unix's epoll binding dump (the AF_UNIX wedge hunt).
    pub epoll_dump_for_file: fn(u64),
    /// The classic-BPF seccomp filter interpreter (syscall crate):
    /// only reached when the current task has a filter installed.
    pub seccomp_run: fn(&[crate::process::task::SockFilter], u64, &crate::arch::pt_regs::PtRegs) -> u32,
    /// The strace1 loader-hunt line formatter (syscall crate):
    /// only reached behind the EVENT_STRACE1 arm gate + pid 1.
    pub strace1_line: fn(u32, &[u64; 6], i64),
}

impl SyscallHooks {
    pub const EMPTY: SyscallHooks = SyscallHooks {
        epoll_interest_snapshot: || {},
        timerfd_wheel_audit: || {},
        pid1_epoll_interest_dump: |_| {},
        kernel_random_bytes: |_| {},
        oh67_fd_ready: |_| None,
        jiffies_covering_deadline: |_| 0,
        deadline_ns_to_hres_ticks: |_| 0,
        timerfd_expire_notify: |_| {},
        param_stall_dump: || {},
        is_eventfd_ops: |_| false,
        syscall_eio_line: |_, _| {},
        epoll_dump_for_file: |_| {},
        seccomp_run: |_, _, _| 0,
        strace1_line: |_, _, _| {},
    };
}

pub static SYSCALL_HOOKS: crate::safety::slot::OnceSlot<SyscallHooks> =
    crate::safety::slot::OnceSlot::new();

/// Registration entry (called by syscall::register_all).
pub fn syscall_register_hooks(hooks: SyscallHooks) {
    let _ = SYSCALL_HOOKS.publish(hooks);
}

#[inline]
fn hooks() -> SyscallHooks {
    *SYSCALL_HOOKS.get().unwrap_or(&SyscallHooks::EMPTY)
}

#[inline]
pub fn epoll_interest_snapshot() { (hooks().epoll_interest_snapshot)() }
#[inline]
pub fn timerfd_wheel_audit() { (hooks().timerfd_wheel_audit)() }
#[inline]
pub fn pid1_epoll_interest_dump(pid: crate::process::task::Pid) { (hooks().pid1_epoll_interest_dump)(pid) }
#[inline]
pub fn kernel_random_bytes(out: &mut [u8]) { (hooks().kernel_random_bytes)(out) }
#[inline]
pub fn oh67_fd_ready(file: &File) -> Option<alloc::string::String> { (hooks().oh67_fd_ready)(file) }
#[inline]
pub fn jiffies_covering_deadline(deadline_ns: u64) -> u64 { (hooks().jiffies_covering_deadline)(deadline_ns) }
#[inline]
pub fn deadline_ns_to_hres_ticks(deadline_ns: u64) -> u64 { (hooks().deadline_ns_to_hres_ticks)(deadline_ns) }
#[inline]
pub fn timerfd_expire_notify(tfd: u64) { (hooks().timerfd_expire_notify)(tfd) }
#[inline]
pub fn param_stall_dump() { (hooks().param_stall_dump)() }
#[inline]
pub fn is_eventfd_ops(ops: &'static crate::fs::FileOps) -> bool { (hooks().is_eventfd_ops)(ops) }
#[inline]
pub fn syscall_eio_line(nr: u64, args: &[u64; 6]) { (hooks().syscall_eio_line)(nr, args) }
#[inline]
pub fn epoll_dump_for_file(fid: u64) { (hooks().epoll_dump_for_file)(fid) }
#[inline]
pub fn seccomp_run(
    prog: &[crate::process::task::SockFilter],
    nr: u64,
    regs: &crate::arch::pt_regs::PtRegs,
) -> u32 {
    (hooks().seccomp_run)(prog, nr, regs)
}
#[inline]
pub fn strace1_line(nr: u32, args: &[u64; 6], result: i64) {
    (hooks().strace1_line)(nr, args, result)
}

// ==================== the mmap ring (moved bodily) ====================

// FORENSIC ring: every mmap call (all sizes — ld.so's second-segment
// mapping is <1MB and was a blind spot of serial-print tracing).
pub struct MmapEntry {
    pub pid: core::sync::atomic::AtomicU32,
    pub addr: core::sync::atomic::AtomicU64,
    pub len: core::sync::atomic::AtomicU64,
    pub flags: core::sync::atomic::AtomicU32,
    pub fd: core::sync::atomic::AtomicI32,
    pub off: core::sync::atomic::AtomicU64,
    pub ret: core::sync::atomic::AtomicU64,
}
impl MmapEntry {
    const fn new() -> Self {
        Self {
            pid: core::sync::atomic::AtomicU32::new(0),
            addr: core::sync::atomic::AtomicU64::new(0),
            len: core::sync::atomic::AtomicU64::new(0),
            flags: core::sync::atomic::AtomicU32::new(0),
            fd: core::sync::atomic::AtomicI32::new(0),
            off: core::sync::atomic::AtomicU64::new(0),
            ret: core::sync::atomic::AtomicU64::new(0),
        }
    }
}
const MMAP_NEW: MmapEntry = MmapEntry::new();
pub static MMAP_RING: [MmapEntry; 1024] = [MMAP_NEW; 1024];
pub static MMAP_CURSOR: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

