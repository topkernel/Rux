//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! /sys/kernel/debug/block/<dev>/ — the blktrace-compat per-device
//! surface over the EXISTING block:block_rq_issue / block:block_rq_complete
//! tracepoints (dfx-align storage row 3).
//!
//! The reference geometry (this host, kernel 7.0.0-42):
//!
//! - `/sys/kernel/debug/block/<dev>/` — per-gendisk debugfs directory
//!   (block/blktrace.c blk_debugfs_register). It carries the hctx
//!   inspection tree (hctx0/cpu*/...rq_list), pm_only, poll_stat,
//!   requeue_list, rqos, sched, state — a request-queue model Rux has
//!   no counterpart for (Rux merges no requests: every buffer-head
//!   transfer IS one request, there are no hctx queues to inspect).
//!   Those files are NOT created here — never fake a queue that does
//!   not exist.
//! - Arming in the reference is the sysfs side: /sys/block/<dev>/trace/
//!   {act_mask, enable, end_lba, pid, start_lba} (host-verified file
//!   list; enable reads "0"). The relay files (trace, msg, dropped)
//!   appear in the debugfs dir only WHILE armed (blk_trace_setup).
//!
//! Rux's surface — two files per registered disk:
//!
//! - `enable` (0644): reads "0"/"1" exactly like the reference sysfs
//!   attr (host: `cat /sys/block/sda/trace/enable` -> 0). Write 1 arms
//!   block:block_rq_issue + block_rq_complete (the trace framework's
//!   own event switches — the same switches tracefs's
//!   events/block/<event>/enable drives, so the last writer wins on a
//!   shared switch) and raises the tracing_on master (records flow
//!   through the shared ring gated by both; the surface never lowers
//!   the master — see set_device_armed). Write 0 disarms when no
//!   device still holds an arm. THE ARMING IS MACHINE-WIDE, not
//!   per-queue: Rux's rings are global, there are no per-device relay
//!   buffers. Per-device separation happens at readback. This is the
//!   surface's one deliberate divergence from the reference layout
//!   (arming in debugfs, not sysfs — the mission's
//!   /sys/kernel/debug/block/<dev>/ surface; a
//!   /sys/block/<dev>/trace/ sysfs attr clone would need the sysfs
//!   per-device attribute tree and adds nothing while the debugfs
//!   file answers the same consumers).
//! - `trace` (0400): the per-device readback — every live-ring record
//!   of the two block events whose dev field matches this device,
//!   rendered by the trace framework's standard record renderer (the
//!   reference tracefs line geometry: TASK-PID [CPU] flags seconds:
//!   event: args). NOT the reference's relay file content: the
//!   reference's debugfs block/<dev>/trace is the blkraw BINARY relay
//!   (rchan buffers) only while armed — Rux has no relay buffers, so
//!   blkparse would reject this file. The text render is the same
//!   geometry the reference's own tracefs `trace` file carries.
//! - No `dropped` file: the reference counts records lost to full
//!   relay buffers; Rux's rings overwrite silently with no drop
//!   counter — a file of invented zeros would lie.

// Kernel layer policy (docs/architecture/kernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

/// One registered disk's blktrace state. `dev` is the tracepoints'
/// dev encoding (major << 20 | minor) — the same value the issue /
/// complete sites record in their first argument slot.
struct BlkTraceDev {
    dev: u32,
    enabled: Arc<AtomicBool>,
}

/// Every disk this surface has seen (registration order). Disks never
/// unregister (Rux has no disk-removal path), so entries are never
/// removed.
static DEVICES: crate::sync::spinlock::Spinlock<Vec<BlkTraceDev>> =
    crate::sync::spinlock::Spinlock::new(Vec::new());

/// Register the per-device blktrace surface for one disk. Called from
/// the disk registration path (blkdev::register_disk, after the
/// /proc/diskstats registration — same name the diskstats row uses).
/// A no-op when debugfs is not mounted yet (early probes before the
/// fs phase): the surface returns false and the disk simply has no
/// debugfs directory — never a panic on the driver path.
pub fn device_registered(major: u32, minor: u32, name: &str) -> bool {
    use crate::fs::debugfs;

    let dev = (major << 20) | minor;
    let dir = alloc::format!("/block/{}", name);
    if !debugfs::create_dir(&dir) {
        return false;
    }

    let enabled = Arc::new(AtomicBool::new(false));
    DEVICES.lock().push(BlkTraceDev {
        dev,
        enabled: enabled.clone(),
    });

    // enable — the reference sysfs attr's 0/1 read shape, the arming
    // write of blktrace's sysfs knob (blk_trace_start_stop).
    let enable_path = alloc::format!("{}/enable", dir);
    let store_enabled = enabled.clone();
    let gen = move || {
        if enabled.load(Ordering::Relaxed) {
            b"1\n".to_vec()
        } else {
            b"0\n".to_vec()
        }
    };
    let store = move |buf: &[u8]| -> i32 {
        let text = core::str::from_utf8(buf).unwrap_or("").trim();
        if text.is_empty() {
            // The guest's `echo v > file` writes the value and the
            // trailing newline as SEPARATE write() calls: the empty
            // second write is a no-op, not an error (the debug=
            // knobs' convention).
            return 0;
        }
        let on = match text {
            "1" => true,
            "0" => false,
            _ => return crate::errno::Errno::InvalidArgument.as_neg_i32(),
        };
        set_device_armed(&store_enabled, on);
        crate::pr_info!(
            "blktrace: device tracing {}",
            if on { "enabled" } else { "disabled" }
        );
        0
    };
    if !debugfs::create_file(&enable_path, 0o644, gen, Some(store)) {
        return false;
    }

    // trace — the per-device record readback (see the module doc for
    // why the content is the tracefs text geometry, not blkraw).
    let trace_path = alloc::format!("{}/trace", dir);
    let gen = move || render_device_records(dev);
    debugfs::create_file(&trace_path, 0o400, gen, None::<fn(&[u8]) -> i32>)
}

/// One device's arming change: flip the device flag, then set the two
/// block events' switches — arm on any 1; disarm on 0 only when no
/// device still holds an arm (the events may be shared with tracefs
/// consumers; a 1 written through events/block/.../enable keeps the
/// records flowing even after every device's 0). Arming also raises
/// the tracing_on master: records flow through the shared ring gated
/// by BOTH switches (the reference's blktrace writes relay buffers
/// behind no master — Rux's single-ring architecture gates everything,
/// and the surface would read back empty without it). The master is
/// never LOWERED here: with the events disarmed an on master costs
/// one relaxed load per record attempt, and lowering it from this
/// surface would silence other consumers' arming.
fn set_device_armed(flag: &Arc<AtomicBool>, on: bool) {
    flag.store(on, Ordering::Relaxed);
    let any = DEVICES.lock().iter().any(|d| d.enabled.load(Ordering::Relaxed));
    for event in [
        &super::EVENT_BLOCK_RQ_ISSUE,
        &super::EVENT_BLOCK_RQ_COMPLETE,
    ] {
        event
            .enabled
            .store(any, core::sync::atomic::Ordering::Relaxed);
    }
    if any {
        super::set_tracing_on(true);
    }
}

/// The per-device readback: every live-ring record of the two block
/// events whose dev slot matches `dev`, in the trace framework's
/// standard record geometry. Allocation-heavy (merges the rings) —
/// a debug-read path, never on the block hot path.
fn render_device_records(dev: u32) -> Vec<u8> {
    let issue = super::EVENT_BLOCK_RQ_ISSUE.id;
    let complete = super::EVENT_BLOCK_RQ_COMPLETE.id;
    let mut out = String::new();
    for record in super::live_records() {
        if record.event_id != issue && record.event_id != complete {
            continue;
        }
        if record.args[0] != dev as u64 {
            continue;
        }
        out.push_str(&super::render_record(&record));
    }
    out.into_bytes()
}
