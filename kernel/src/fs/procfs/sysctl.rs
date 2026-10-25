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

/// /proc/sys/kernel/tainted — taint-flag bitmap.
///
/// Linux's format is NUMERIC ("Non-zero if the kernel has been tainted.
/// Numeric values, which can be ORed together.") — the letters appear
/// in the "Tainted:" line of WARN/BUG/panic output, not here. The value
/// is the live `dfx::taint` mask, so a WARN or a soft lockup shows up
/// immediately. LTP's tst_taint reads it before/after each test to
/// detect kernel bugs (timerfd_settime02 aborts TBROK when the file is
/// missing).
pub fn generate_tainted() -> Vec<u8> {
    alloc::format!("{}\n", crate::dfx::taint::get_taints()).into_bytes()
}


/// /proc/sys/kernel/panic_on_oops — dies on oops instead of killing the
/// faulting task (kernel/panic.c's oops=panic seed, sysctl runtime
/// knob). Default 0, like the reference; the DFX lane's oops path reads
/// it through dfx::oops::panic_on_oops().
pub fn generate_panic_on_oops() -> Vec<u8> {
    alloc::format!(
        "{}\n",
        if crate::dfx::oops::panic_on_oops() { 1 } else { 0 }
    )
    .into_bytes()
}

pub fn write_panic_on_oops(input: &[u8]) -> i32 {
    match parse_u64(input) {
        Ok(v @ 0..=1) => {
            crate::dfx::oops::set_panic_on_oops(v == 1);
            0
        }
        _ => -(crate::errno::constants::EINVAL as i32),
    }
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

/// /proc/sys/fs/pipe-max-size — largest pipe capacity an unprivileged
/// F_SETPIPE_SZ may request (Linux default 1MB). Read by LTP fcntl30;
/// consulted by Pipe::resize as the ceiling.
pub static PIPE_MAX_SIZE: AtomicU64 = AtomicU64::new(1 << 20);

pub fn generate_pipe_max_size() -> Vec<u8> {
    alloc::format!("{}\n", PIPE_MAX_SIZE.load(Ordering::Relaxed)).into_bytes()
}

pub fn write_pipe_max_size(input: &[u8]) -> i32 {
    match parse_u64(input) {
        Ok(v) => {
            // Linux clamps to [page, UINT_MAX]; below one page is EINVAL.
            if v < 4096 {
                return -(crate::errno::constants::EINVAL as i32);
            }
            PIPE_MAX_SIZE.store(v, Ordering::Release);
            0
        }
        Err(e) => e,
    }
}

/// /proc/sys/fs/lease-break-time — seconds to wait after signalling a
/// lease holder before breaking the lease by force (Linux default 45;
/// read+written by LTP fcntl33).
pub fn generate_lease_break_time() -> Vec<u8> {
    alloc::format!(
        "{}\n",
        crate::fs::locks::LEASE_BREAK_TIME.load(Ordering::Relaxed)
    )
    .into_bytes()
}

pub fn write_lease_break_time(input: &[u8]) -> i32 {
    match parse_u64(input) {
        Ok(v) => {
            if v > i32::MAX as u64 {
                return -(crate::errno::constants::EINVAL as i32);
            }
            crate::fs::locks::LEASE_BREAK_TIME.store(v as u32, Ordering::Release);
            0
        }
        Err(e) => e,
    }
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
    alloc::format!("{}\n", crate::ipc::sysv_msg::msgmni()).into_bytes()
}

/// /proc/sys/kernel/msgmni write — LTP msgget03 lowers MSGMNI and expects
/// the next msgget to fail ENOSPC.
pub fn write_msgmni(input: &[u8]) -> i32 {
    match parse_u64(input) {
        Ok(v) => {
            if v > i32::MAX as u64 || !crate::ipc::sysv_msg::set_msgmni(v as i32) {
                return -(crate::errno::constants::EINVAL as i32);
            }
            0
        }
        Err(e) => e,
    }
}

/// /proc/sys/kernel/threads-max — max threads system-wide (read by LTP
/// tst_pid / msgstress01).
pub fn generate_threads_max() -> Vec<u8> {
    // Linux default for a 2GB machine (mempages / (8 * THREAD_SIZE/PAGE));
    // tst_max_child_pids() scales fork workloads off this.
    alloc::format!("31418\n").into_bytes()
}

/// /proc/sys/kernel/printk — four loglevels as on Linux:
/// `<console_loglevel> <default_message_loglevel> <minimum_console_loglevel>
/// <default_console_loglevel>`. The first number is live: syslog(2)
/// actions 6/7/8 store into the same CONSOLE_LOGLEVEL this reads, so LTP
/// syslog11 can observe level changes through the file.
pub fn generate_printk() -> Vec<u8> {
    let cur = crate::printk::get_console_loglevel();
    alloc::format!("{} 4 1 7\n", cur).into_bytes()
}

/// Write side: the first integer sets the console loglevel (Linux
/// printk_sysctl bounds 1..=15; single-number writes are what sysctl(8)
/// and LTP's save/restore emit).
pub fn write_printk(input: &[u8]) -> i32 {
    match parse_u64(input) {
        Ok(v) => {
            if !(1..=15).contains(&v) {
                return -(crate::errno::constants::EINVAL as i32);
            }
            crate::printk::set_console_loglevel(v as u8);
            0
        }
        Err(e) => e,
    }
}

/// /proc/sys/kernel/shmmax — max segment size. Live: the shmget size
/// check and shmctl(IPC_INFO) both consult it (writable; LTP shmget02
/// lowers it to 8192, shmctl03 asserts IPC_INFO equality).
pub fn generate_shmmax() -> Vec<u8> {
    alloc::format!("{}\n", crate::ipc::sysv_shm::shm_ctlmax()).into_bytes()
}

pub fn write_shmmax(input: &[u8]) -> i32 {
    match parse_u64(input) {
        Ok(v) => {
            crate::ipc::sysv_shm::SHM_CTLMAX.store(v, Ordering::Release);
            0
        }
        Err(e) => e,
    }
}

/// /proc/sys/kernel/shmmni — max segments system-wide (LTP shmget03 reads
/// it and creates exactly that many segments).
pub fn generate_shmmni() -> Vec<u8> {
    alloc::format!("{}\n", crate::ipc::sysv_shm::shm_ctlni()).into_bytes()
}

pub fn write_shmmni(input: &[u8]) -> i32 {
    match parse_u64(input) {
        Ok(v) => {
            // The fixed slot table caps the real limit at IPC_IDS_MAX;
            // clamp so the file never promises more than the kernel has.
            let capped = v.min(crate::ipc::util::IPC_IDS_MAX as u64);
            crate::ipc::sysv_shm::SHM_CTLNI.store(capped, Ordering::Release);
            0
        }
        Err(e) => e,
    }
}

/// /proc/sys/kernel/shmall — total shm page budget (ENOSPC past it).
pub fn generate_shmall() -> Vec<u8> {
    alloc::format!("{}\n", crate::ipc::sysv_shm::shm_ctlall()).into_bytes()
}

pub fn write_shmall(input: &[u8]) -> i32 {
    match parse_u64(input) {
        Ok(v) => {
            crate::ipc::sysv_shm::SHM_CTLALL.store(v, Ordering::Release);
            0
        }
        Err(e) => e,
    }
}

/// /proc/sys/fs/mqueue/queues_max — writable POSIX mq count limit.
pub fn generate_mq_queues_max() -> Vec<u8> {
    alloc::format!("{}\n", crate::ipc::posix_mq::queues_max()).into_bytes()
}

pub fn write_mq_queues_max(input: &[u8]) -> i32 {
    match parse_u64(input) {
        Ok(v) => {
            if v > i32::MAX as u64 || !crate::ipc::posix_mq::set_queues_max(v as i32) {
                return -(crate::errno::constants::EINVAL as i32);
            }
            0
        }
        Err(e) => e,
    }
}

/// /proc/sys/fs/mqueue/msg_max — max messages per queue (read-only here).
pub fn generate_mq_msg_max() -> Vec<u8> {
    alloc::format!("10\n").into_bytes()
}

/// /proc/sys/fs/mqueue/msgsize_max — max message size (read-only here).
pub fn generate_mq_msgsize_max() -> Vec<u8> {
    alloc::format!("8192\n").into_bytes()
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

// ============================================================================
// /proc/sys/fs/inotify — read by LTP inotify05/06/12 and glib's GFileMonitor
// ============================================================================

/// /proc/sys/fs/inotify/max_queued_events — per-instance event-queue cap.
/// Live: the inotify enqueue path consults it (rw; LTP inotify05 reads it
/// and relies on the IN_Q_OVERFLOW-last semantics when it is exceeded).
pub fn generate_inotify_max_queued_events() -> Vec<u8> {
    alloc::format!("{}\n", crate::fs::inotify::max_queued_events()).into_bytes()
}

pub fn write_inotify_max_queued_events(input: &[u8]) -> i32 {
    match parse_u64(input) {
        Ok(v) => {
            // Linux accepts >= 0 (0 means "overflow on the first event");
            // cap at a sane bound to keep the queue a VecDeque of events.
            if v > (1 << 20) {
                return -(crate::errno::constants::EINVAL as i32);
            }
            crate::fs::inotify::MAX_QUEUED_EVENTS_SYSCTL
                .store(v as usize, Ordering::Release);
            0
        }
        Err(e) => e,
    }
}

/// /proc/sys/fs/inotify/max_user_instances — per-user instance cap
/// (Linux default 128). Stored only: no per-user accounting exists yet;
/// inotify06 raises the limit before its churn loop.
pub static INOTIFY_MAX_USER_INSTANCES: AtomicU64 = AtomicU64::new(128);

pub fn generate_inotify_max_user_instances() -> Vec<u8> {
    alloc::format!("{}\n", INOTIFY_MAX_USER_INSTANCES.load(Ordering::Relaxed)).into_bytes()
}

pub fn write_inotify_max_user_instances(input: &[u8]) -> i32 {
    match parse_u64(input) {
        Ok(v) => {
            INOTIFY_MAX_USER_INSTANCES.store(v, Ordering::Release);
            0
        }
        Err(e) => e,
    }
}

/// /proc/sys/fs/inotify/max_user_watches — per-user watch cap (Linux
/// default 1048576). Stored only for now (no per-user accounting);
/// raising it is what userland (inotify06, desktop file managers) does.
pub static INOTIFY_MAX_USER_WATCHES: AtomicU64 = AtomicU64::new(1048576);

pub fn generate_inotify_max_user_watches() -> Vec<u8> {
    alloc::format!("{}\n", INOTIFY_MAX_USER_WATCHES.load(Ordering::Relaxed)).into_bytes()
}

pub fn write_inotify_max_user_watches(input: &[u8]) -> i32 {
    match parse_u64(input) {
        Ok(v) => {
            INOTIFY_MAX_USER_WATCHES.store(v, Ordering::Release);
            0
        }
        Err(e) => e,
    }
}
