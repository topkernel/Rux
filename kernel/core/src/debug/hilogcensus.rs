//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! rux:hilogcensus (retired debug=hilogcensus) — per-caller census of the OpenHarmony log fabric
//! (hilog-flood lane).
//!
//! The OH boot floods the hilog unix-dgram fabric at 10^4-10^6 msg/s
//! (hilogd.server recvmsg matches the writers' writev ok count 1:1 —
//! every message is one writev(header, tag, content) datagram from
//! libhilog). syscost's 16-slot first-seen comm table cannot name the
//! producers (slots fill early, the big writers print past the printk
//! line truncation), and the in-guest `hilog` CLI returns nothing on
//! this image. This census answers "WHO emits, WHAT tag, at what rate"
//! from the kernel side:
//!
//! - `unix_send_pre/post` + the periodic dump — MOVED to the debug
//!   service crate (kernel/debug/src/hilogcensus.rs; root-cold lane
//!   2026-11-24): syscall's writev path calls them there directly
//!   (sibling service dep, zero indirection), and the dump fires
//!   through the DebugHooks `hilog_dump` row below.
//! - `kmsg_write(buf)` — stays HERE: called by name from printk.rs's
//!   kmsg_file_write and the ext4 journal seam (both framework files),
//!   so rule 1/2 pins it. The kmsg table stays with it and the
//!   crate-side dump reads it through the pub surface below (the
//!   syscost/fws split-file pattern: markers framework-side, report
//!   halves crate-side).
//!
//! Arming (obs-hygiene migration): the unified-framework tracepoint
//! `rux:hilog_census` IS the switch — arm it via `trace_event=` on the
//! cmdline or `set_event` at runtime; one relaxed load per hook when
//! armed, zero when off. NOT `hilogtap`, which arms the existing
//! per-record content tap in net/unix.rs — that scanner costs ~seconds
//! of CPU/s at storm rates. The bespoke cmdline token `hilogcensus=1`
//! survives as a compatibility ALIAS that arms the same events at boot
//! (existing hunt rigs keep working; the frozen debug= switch table is
//! untouched — this module was never in it). Dumped on the syscost
//! periodic cadence (and the shutdown cascade): top senders, one
//! bounded line each. Aggregate-as-event throughout — a per-message
//! tracepoint at 10^6/s would cost more than it observes.
//!
//! Table honesty notes: unix slots are DIRECT-MAPPED by
//! `sender_pid & (SLOTS-1)`; a colliding/reused pid resets the slot
//! (EVICT counts the resets). Rate = n delta between dumps / dump
//! interval, computed host-side from the printed cumulative counts.

// Kernel layer policy (docs/architecture/kernel-layers.md): this module
// is data-plane Safe Rust EXCEPT the read-only `current task`
// comm/pid snapshot in kmsg_write (the syscost flood_account pattern —
// the same single documented unsafe shape, no other unsafe here).

#![forbid(unsafe_code)]
use core::sync::atomic::Ordering::Relaxed;
use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8};

/// The census gate: the unified-framework event switch (live — arming
/// through tracefs `set_event` at runtime takes effect immediately,
/// which the old cached tri-state could not do).
#[inline]
pub fn on() -> bool {
    crate::trace::EVENT_HILOG_CENSUS.armed()
}

/// Compatibility arm (boot, before trace_event= is applied): the old
/// bespoke cmdline token `hilogcensus=1` now just flips the unified
/// events' enable switches — same running state as
/// `trace_event=rux:hilog_census,rux:hilog_kmsg`.
pub fn compat_arm() {
    if crate::cmdline::get_cmdline().is_none() {
        return;
    }
    if crate::cmdline::get_param("hilogcensus").as_deref() == Some("1") {
        crate::trace::EVENT_HILOG_CENSUS.enabled.store(true, Relaxed);
        crate::trace::EVENT_HILOG_KMSG.enabled.store(true, Relaxed);
        crate::pr_warn!(
            "hilogcensus: compat token hilogcensus=1 armed (rux:hilog_census + rux:hilog_kmsg)"
        );
    }
}

const COMM: usize = 16;

/// The /dev/kmsg writer row (read by the crate-side dump through this
/// pub surface — the kmsg marker stayed framework-side with printk's
/// call site).
pub struct KmsgSender {
    pub pid: AtomicU32,
    pub n: AtomicU64,
    pub comm: [AtomicU8; COMM],
    /// Last line head (raw, ≤32 bytes) — the repeating-pattern sample.
    pub head: [AtomicU8; 32],
    pub head_len: AtomicU32,
}

impl KmsgSender {
    const fn new() -> Self {
        KmsgSender {
            pid: AtomicU32::new(u32::MAX),
            n: AtomicU64::new(0),
            comm: [const { AtomicU8::new(0) }; COMM],
            head: [const { AtomicU8::new(0) }; 32],
            head_len: AtomicU32::new(0),
        }
    }
}

pub const KMSG_SLOTS: usize = 256;
pub static KMSG: [KmsgSender; KMSG_SLOTS] = [const { KmsgSender::new() }; KMSG_SLOTS];

/// Hook: one userspace /dev/kmsg write (printk.rs kmsg_file_write).
pub fn kmsg_write(buf: &[u8]) {
    if !on() {
        return;
    }
    let (spid, scomm): (u32, [u8; COMM]) = match crate::sched::current() {
        // sched::current() returns a typed &mut Task reference.
        Some(t) => (t.pid() as u32, *t.comm()),
        None => return,
    };
    let slot = &KMSG[(spid as usize) & (KMSG_SLOTS - 1)];
    if slot.pid.load(Relaxed) != spid {
        slot.pid.store(spid, Relaxed);
        slot.n.store(0, Relaxed);
        for j in 0..COMM {
            slot.comm[j].store(scomm[j], Relaxed);
        }
    }
    slot.n.fetch_add(1, Relaxed);
    let hl = buf.len().min(32);
    for j in 0..hl {
        slot.head[j].store(buf[j], Relaxed);
    }
    slot.head_len.store(hl as u32, Relaxed);
}

/// Periodic dump (syscost cadence + shutdown cascade) — the body lives
/// in the debug crate; this wrapper dispatches the DebugHooks
/// `hilog_dump` row (process's reboot cascade and any framework
/// site keep the historical name).
pub fn dump() {
    crate::debug::debug_hooks::hilog_dump();
}
