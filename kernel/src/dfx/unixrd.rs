//! dfx=unixrd — AF_UNIX hot-path per-op phase anatomy probe (39th lane).
//!
//! The hilogd storm measures recvmsg 28-58 us / recvfrom 9 us / writev
//! 22 us per-op (syscost per-nr tables) against the reference kernel's
//! ~2-5 us on the same flood — a 90k/s socket-op storm that saturates
//! both CPUs and starves the cooperative FFRT userspace into the wedge
//! cascade (38th close, u2v6). The syscost per-nr averages bracket the
//! WHOLE syscall; they cannot say WHERE inside the call the time goes.
//! This probe decomposes the three unix hot entries into phases and
//! reports cycle totals per phase on the syscost dump cadence:
//!
//! recvmsg AF_UNIX branch (RM_*):
//! - `outer` — the whole AF_UNIX branch (fd->socket resolve to return)
//! - `iov`   — the shared iovec snapshot loop (all families; the storm
//!             is unix-dominated so the attribution holds)
//! - `alloc` — the kernel bounce buffer `vec![0u8; total_buf_len]`
//!             (all families; `allocb` = summed byte request)
//! - `recv`  — unix_recv_ctl: queue lock, drain copy, wake/notify
//! - `cout`  — the scatter copy_to_user back to the iovecs
//! - `cmsg`  — deliver_unix_cmsgs (SCM_RIGHTS install + SCM_CREDENTIALS
//!             emission; `cred` = calls that emitted a cred cmsg)
//! - `addr`  — put_sockaddr_un source-address write-back
//!
//! recvfrom AF_UNIX branch (RF_*): the hilogd 2-byte length peek rides
//! this entry (`peek` count). Phases: outer/alloc/recv/cout.
//!
//! writev gathered-dgram path (WV_*): libhilog sends every record as
//! writev(header, tag, content) on its connected DGRAM — one gather
//! allocation + copy_from_user per iovec, then the owned send (queue
//! push + cred stamp + wake). Phases: outer/gather/send.
//!
//! All sums are raw monotonic-ns; per-op averages = sum / n. Hooks sit
//! behind one `unixrd=1` cmdline param (cctx tri-state pattern): one
//! relaxed load per call when on, zero cost when off. OFF by default.
//!
//! Dumped from syscost's periodic dump (45s-mod-60 cadence) and the
//! shutdown cascade — the wedge-era machine never reaches reboot(2)
//! cleanly under the boot driver, so the periodic line is the evidence
//! channel.

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use core::sync::atomic::Ordering::Relaxed;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8};

static ENABLED: AtomicBool = AtomicBool::new(false);

// === recvmsg AF_UNIX =======================================================

pub static RM_N: AtomicU64 = AtomicU64::new(0);
/// AF_UNIX recvmsg that returned data.
pub static RM_OK_N: AtomicU64 = AtomicU64::new(0);
/// AF_UNIX recvmsg that returned EAGAIN (empty queue).
pub static RM_EA_N: AtomicU64 = AtomicU64::new(0);
/// AF_UNIX recvmsg that returned another error.
pub static RM_ERR_N: AtomicU64 = AtomicU64::new(0);
/// MSG_PEEK variant calls.
pub static RM_PEEK_N: AtomicU64 = AtomicU64::new(0);
/// Calls whose cmsg chain carried SCM_CREDENTIALS (receiver SO_PASSCRED).
pub static RM_CRED_N: AtomicU64 = AtomicU64::new(0);

pub static RM_OUTER_NS: AtomicU64 = AtomicU64::new(0);
pub static RM_IOV_NS: AtomicU64 = AtomicU64::new(0);
pub static RM_ALLOC_NS: AtomicU64 = AtomicU64::new(0);
/// Byte request behind RM_ALLOC (sum of total_buf_len).
pub static RM_ALLOC_B: AtomicU64 = AtomicU64::new(0);
pub static RM_RECV_NS: AtomicU64 = AtomicU64::new(0);
pub static RM_COUT_NS: AtomicU64 = AtomicU64::new(0);
pub static RM_CMSG_NS: AtomicU64 = AtomicU64::new(0);
pub static RM_ADDR_NS: AtomicU64 = AtomicU64::new(0);

// === recvfrom AF_UNIX ======================================================

pub static RF_N: AtomicU64 = AtomicU64::new(0);
pub static RF_OK_N: AtomicU64 = AtomicU64::new(0);
pub static RF_EA_N: AtomicU64 = AtomicU64::new(0);
pub static RF_ERR_N: AtomicU64 = AtomicU64::new(0);
pub static RF_PEEK_N: AtomicU64 = AtomicU64::new(0);

pub static RF_OUTER_NS: AtomicU64 = AtomicU64::new(0);
pub static RF_ALLOC_NS: AtomicU64 = AtomicU64::new(0);
/// Byte request behind RF_ALLOC.
pub static RF_ALLOC_B: AtomicU64 = AtomicU64::new(0);
pub static RF_RECV_NS: AtomicU64 = AtomicU64::new(0);
pub static RF_COUT_NS: AtomicU64 = AtomicU64::new(0);

// === writev gathered-dgram =================================================

pub static WV_N: AtomicU64 = AtomicU64::new(0);
pub static WV_OK_N: AtomicU64 = AtomicU64::new(0);
/// Sender-side EAGAIN (target queue full / nonblock).
pub static WV_EA_N: AtomicU64 = AtomicU64::new(0);
pub static WV_ERR_N: AtomicU64 = AtomicU64::new(0);

pub static WV_OUTER_NS: AtomicU64 = AtomicU64::new(0);
pub static WV_GATHER_NS: AtomicU64 = AtomicU64::new(0);
/// Gathered byte total behind WV_GATHER.
pub static WV_GATHER_B: AtomicU64 = AtomicU64::new(0);
pub static WV_SEND_NS: AtomicU64 = AtomicU64::new(0);

/// Bracket timestamp (0 when the probe is off — skips the clock read).
/// Calls on() so the FIRST call after cmdline init resolves the tri-state
/// (one relaxed load per call afterwards).
#[inline]
pub fn now() -> u64 {
    if on() {
        crate::drivers::timer::monotonic_ns_since_boot()
    } else {
        0
    }
}

/// Close a bracket opened with now() (no-op when t0 == 0, i.e. off).
#[inline]
pub fn add(dst: &AtomicU64, t0: u64) {
    if t0 != 0 {
        dst.fetch_add(
            crate::drivers::timer::monotonic_ns_since_boot().saturating_sub(t0),
            Relaxed,
        );
    }
}

/// Bump a count (no-op when off — resolves the tri-state like now()).
#[inline]
pub fn bump(c: &AtomicU64) {
    if on() {
        c.fetch_add(1, Relaxed);
    }
}

/// Byte-sum helper (no-op when off).
#[inline]
pub fn addb(dst: &AtomicU64, bytes: u64) {
    if on() {
        dst.fetch_add(bytes, Relaxed);
    }
}

/// Probe armed? Tri-state exactly like cctx (0 = unresolved, 1 = off,
/// 2 = on); never cache a verdict before the cmdline exists.
static ARMED8: AtomicU8 = AtomicU8::new(0);

#[inline]
pub fn on() -> bool {
    match ARMED8.load(Relaxed) {
        2 => true,
        1 => false,
        _ => {
            let v = match crate::cmdline::get_cmdline() {
                None => return false,
                Some(_) => crate::cmdline::get_param("unixrd").as_deref() == Some("1"),
            };
            ARMED8.store(if v { 2 } else { 1 }, Relaxed);
            ENABLED.store(v, Relaxed);
            crate::pr_warn!(
                "unixrd: {} (cmdline unixrd param = {:?})",
                if v { "armed" } else { "off" },
                crate::cmdline::get_param("unixrd")
            );
            v
        }
    }
}

fn v(c: &AtomicU64) -> u64 {
    c.load(Relaxed)
}

/// Per-op average in ns (0 when n == 0).
fn avg(sum: &AtomicU64, n: &AtomicU64) -> u64 {
    let n = v(n);
    if n == 0 {
        0
    } else {
        v(sum) / n
    }
}

/// Print the phase snapshot (syscost periodic dump + shutdown cascade).
pub fn dump() {
    if !on() {
        return;
    }
    let secs = crate::drivers::timer::get_jiffies() / crate::config::KERNEL_HZ as u64;
    // recvmsg AF_UNIX phases. iov/alloc sum over ALL recvmsg families
    // (shared code above the family dispatch) — the storm boot is
    // unix-dominated, so sum/N vs the AF_UNIX count reads as an upper
    // bound; the informative number is the RATIO of phases.
    crate::pr_warn!(
        "UNIXRD-RM t={}s n={} ok={} ea={} err={} peek={} cred={} outer={}ns iov={}ns alloc={}ns recv={}ns cout={}ns cmsg={}ns addr={}ns allocb={}B",
        secs,
        v(&RM_N),
        v(&RM_OK_N),
        v(&RM_EA_N),
        v(&RM_ERR_N),
        v(&RM_PEEK_N),
        v(&RM_CRED_N),
        avg(&RM_OUTER_NS, &RM_N),
        avg(&RM_IOV_NS, &RM_N),
        avg(&RM_ALLOC_NS, &RM_N),
        avg(&RM_RECV_NS, &RM_N),
        avg(&RM_COUT_NS, &RM_N),
        avg(&RM_CMSG_NS, &RM_N),
        avg(&RM_ADDR_NS, &RM_N),
        if v(&RM_N) == 0 { 0 } else { v(&RM_ALLOC_B) / v(&RM_N) },
    );
    crate::pr_warn!(
        "UNIXRD-RF t={}s n={} ok={} ea={} err={} peek={} outer={}ns alloc={}ns recv={}ns cout={}ns allocb={}B",
        secs,
        v(&RF_N),
        v(&RF_OK_N),
        v(&RF_EA_N),
        v(&RF_ERR_N),
        v(&RF_PEEK_N),
        avg(&RF_OUTER_NS, &RF_N),
        avg(&RF_ALLOC_NS, &RF_N),
        avg(&RF_RECV_NS, &RF_N),
        avg(&RF_COUT_NS, &RF_N),
        if v(&RF_N) == 0 { 0 } else { v(&RF_ALLOC_B) / v(&RF_N) },
    );
    crate::pr_warn!(
        "UNIXRD-WV t={}s n={} ok={} ea={} err={} outer={}ns gather={}ns send={}ns gatherb={}B",
        secs,
        v(&WV_N),
        v(&WV_OK_N),
        v(&WV_EA_N),
        v(&WV_ERR_N),
        avg(&WV_OUTER_NS, &WV_N),
        avg(&WV_GATHER_NS, &WV_N),
        avg(&WV_SEND_NS, &WV_N),
        if v(&WV_N) == 0 { 0 } else { v(&WV_GATHER_B) / v(&WV_N) },
    );
}
