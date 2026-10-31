//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! dfx=scmtap — SCM_RIGHTS relay tap (gdm session-fifo hunt, session 21).
//!
//! The greeter session dies milliseconds after CreateSession: logind
//! sends the reply carrying the session fifo's write fd, the dbus-daemon
//! must relay it to pam_systemd inside gdm-session-worker, and pam's
//! debug output shows the reply NEVER arrives ("Reply from logind" is
//! absent while logind's "Sending reply about created session ...
//! session_fd=N" prints every attempt). The boot-time fd probes prove
//! the kernel primitives in isolation, so the loss is somewhere in the
//! three-hop relay (logind sendmsg → daemon recvmsg → daemon sendmsg →
//! pam recvmsg) or in the drop paths that close the passed fd.
//!
//! This tap prints one line per hop, from the kernel side:
//!
//! - `SCMTAP TX` — a sendmsg attached fds (sender pid/comm, count).
//! - `SCMTAP TXERR` — a sendmsg's cmsg parse failed (errno).
//! - `SCMTAP RX` — a recvmsg installed fds (receiver pid/comm, the NEW
//!   fd numbers).
//! - `SCMTAP DROP-NOCTL` — receiver offered no control room: the fds
//!   were closed and MSG_CTRUNC reported (the daemon treats CTRUNC as
//!   an IO error and KILLS the connection — the once-per-boot
//!   connection-drop class).
//! - `SCMTAP DROP-EFAULT` / `DROP-EMFILE` — the other delivery
//!   failures; the fds are closed there too.
//! - `SCMTAP ORPHAN` — a socket closed with fds still queued: the
//!   receiver died (or stopped reading) and the queued references were
//!   released.
//! - `SCMTAP FIFO-LASTWRITER` — the last writer of a fifo closed it
//!   (the exact moment logind's EPOLLHUP fires): names the closing
//!   process and the fifo inode.
//!
//! Every line is behind the `dfx=scmtap` switch; cost when off is one
//! relaxed load per potential print site.

// Framekernel layer policy (docs/architecture/framekernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

/// Whether the tap is armed (`dfx=scmtap`).
pub fn enabled() -> bool {
    super::switches::enabled(super::switches::DfxSwitch::ScmTap)
}

/// Current task's (pid, comm) for the tap lines; (0, "?") outside
/// task context.
fn who() -> (u32, &'static str) {
    let comm = crate::sched::current()
        .map(|t| {
            core::str::from_utf8(t.comm())
                .unwrap_or("?")
                .split('\0')
                .next()
                .unwrap_or("?")
        })
        .unwrap_or("?");
    (crate::process::current_pid(), comm)
}

/// TX: `sendmsg` attached `n` fds.
pub fn tap_tx(n: usize) {
    let (pid, comm) = who();
    crate::pr_info!("SCMTAP TX pid={}({}) nfds={}", pid, comm, n);
}

/// TX parse failure: the sendmsg returned before queuing anything.
pub fn tap_tx_err(err: i64) {
    let (pid, comm) = who();
    crate::pr_info!("SCMTAP TXERR pid={}({}) err={}", pid, comm, err);
}

/// RX: `recvmsg` installed these new fds.
pub fn tap_rx(new_fds: &[i32]) {
    let (pid, comm) = who();
    crate::pr_info!("SCMTAP RX pid={}({}) fds={:?}", pid, comm, new_fds);
}

/// RX failure: the attached fds were closed instead of delivered.
pub fn tap_rx_drop(reason: &str, n: usize) {
    let (pid, comm) = who();
    crate::pr_info!(
        "SCMTAP DROP-{} pid={}({}) nfds={}",
        reason,
        pid,
        comm,
        n
    );
}

/// A socket closed with fds still queued in its receive queue.
pub fn tap_orphan(nfds: usize, segs: usize) {
    let (pid, comm) = who();
    crate::pr_info!(
        "SCMTAP ORPHAN pid={}({}) nfds={} segs={}",
        pid,
        comm,
        nfds,
        segs
    );
}

/// The last writer of a fifo closed it — logind's EPOLLHUP moment.
pub fn tap_fifo_last_writer(fs_id: u64, ino: u64) {
    let (pid, comm) = who();
    crate::pr_info!(
        "SCMTAP FIFO-LASTWRITER pid={}({}) fifo={}:{}",
        pid,
        comm,
        fs_id,
        ino
    );
}
