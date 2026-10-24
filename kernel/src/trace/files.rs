//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The tracefs FILES — the filesystem side of the tracing subsystem.
//!
//! Linux's tracefs carries the tracing control surface:
//!
//! ```text
//! /sys/kernel/debug/tracing/
//! ├── tracing_on            1        (the master switch)
//! ├── trace                 <the collected records>
//! ├── available_tracers     nop
//! ├── current_tracer        nop
//! └── events/
//!     └── syscalls/
//!         ├── enable                (the group switch)
//!         └── sys_enter/
//!             ├── enable
//!             ├── id
//!             └── format
//! ```
//!
//! Rux publishes the same layout under `/sys/kernel/debug/tracing`
//! (the path Linux's own documentation uses for tracefs; the kernel
//! also accepts the newer `/sys/kernel/tracing`).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::fs::debugfs;
use crate::trace::{self, TraceEvent};

/// The directory the tracing files live in.
const TRACING_DIR: &str = "/tracing";

/// Parse a "0"/"1" (Linux's boolean file convention).
fn parse_bool(buf: &[u8]) -> Option<bool> {
    let text = core::str::from_utf8(buf).ok()?.trim();
    match text {
        "0" | "N" | "n" | "false" => Some(false),
        "1" | "Y" | "y" | "true" => Some(true),
        _ => None,
    }
}

/// Publish the tracing files.
///
/// Returns the number of files created.
pub fn register() -> usize {
    let mut count = 0usize;

    if !debugfs::create_dir(TRACING_DIR) {
        return 0;
    }

    // --- the master switch ------------------------------------------
    if debugfs::create_file(
        &format!("{}/tracing_on", TRACING_DIR),
        0o644,
        || {
            let mut out = alloc::vec![b'0'];
            if trace::tracing_on() {
                out[0] = b'1';
            }
            out.push(b'\n');
            out
        },
        Some(|buf: &[u8]| match parse_bool(buf) {
            Some(on) => {
                trace::set_tracing_on(on);
                0
            }
            None => crate::errno::Errno::InvalidArgument.as_neg_i32(),
        }),
    ) {
        count += 1;
    }

    // --- the collected records ---------------------------------------
    if debugfs::create_file(
        &format!("{}/trace", TRACING_DIR),
        0o644,
        || trace::render_trace().into_bytes(),
        // Linux: writing to the trace file clears the buffer.
        Some(|_buf: &[u8]| {
            trace::clear();
            0
        }),
    ) {
        count += 1;
    }

    // --- the tracers --------------------------------------------------
    // Rux has no tracer of its own: the events are the whole facility,
    // which is what Linux's "nop" tracer means — it records nothing by
    // itself, the tracepoints do.
    if debugfs::create_file(
        &format!("{}/available_tracers", TRACING_DIR),
        0o444,
        || b"nop\n".to_vec(),
        None::<fn(&[u8]) -> i32>,
    ) {
        count += 1;
    }
    if debugfs::create_file(
        &format!("{}/current_tracer", TRACING_DIR),
        0o644,
        || b"nop\n".to_vec(),
        Some(|buf: &[u8]| {
            let text = core::str::from_utf8(buf).unwrap_or("").trim();
            if text == "nop" {
                0
            } else {
                // Linux refuses an unknown tracer.
                crate::errno::Errno::InvalidArgument.as_neg_i32()
            }
        }),
    ) {
        count += 1;
    }

    // --- the event tree ------------------------------------------------
    for event in trace::EVENTS.iter() {
        let group_dir = format!("{}/events/{}", TRACING_DIR, event.group);
        let event_dir = format!("{}/{}", group_dir, event.name);
        if !debugfs::create_dir(&group_dir) || !debugfs::create_dir(&event_dir) {
            continue;
        }

        // The per-event switch.
        if debugfs::create_file(
            &format!("{}/enable", event_dir),
            0o644,
            || {
                let mut out = alloc::vec![b'0'];
                if event.enabled.load(core::sync::atomic::Ordering::Relaxed) {
                    out[0] = b'1';
                }
                out.push(b'\n');
                out
            },
            Some(|buf: &[u8]| match parse_bool(buf) {
                Some(on) => {
                    event
                        .enabled
                        .store(on, core::sync::atomic::Ordering::Relaxed);
                    0
                }
                None => crate::errno::Errno::InvalidArgument.as_neg_i32(),
            }),
        ) {
            count += 1;
        }

        // The event id.
        if debugfs::create_file(
            &format!("{}/id", event_dir),
            0o444,
            || alloc::format!("{}\n", event.id).into_bytes(),
            None::<fn(&[u8]) -> i32>,
        ) {
            count += 1;
        }

        // The format description.
        if debugfs::create_file(
            &format!("{}/format", event_dir),
            0o444,
            || event.format_file().into_bytes(),
            None::<fn(&[u8]) -> i32>,
        ) {
            count += 1;
        }

        // The group switch (all the events of the group).
        if debugfs::create_file(
            &format!("{}/enable", group_dir),
            0o644,
            || {
                let mut out = alloc::vec![b'0'];
                for other in trace::EVENTS.iter() {
                    if other.group == event.group
                        && other.enabled.load(core::sync::atomic::Ordering::Relaxed)
                    {
                        out[0] = b'1';
                        break;
                    }
                }
                out.push(b'\n');
                out
            },
            Some(|buf: &[u8]| match parse_bool(buf) {
                Some(on) => {
                    for other in trace::EVENTS.iter() {
                        if other.group == event.group {
                            other
                                .enabled
                                .store(on, core::sync::atomic::Ordering::Relaxed);
                        }
                    }
                    0
                }
                None => crate::errno::Errno::InvalidArgument.as_neg_i32(),
            }),
        ) {
            count += 1;
        }
    }

    count
}

/// The events of one group (used by the group switch).
pub fn group_events(group: &str) -> Vec<&'static TraceEvent> {
    trace::EVENTS
        .iter()
        .filter(|e| e.group == group)
        .copied()
        .collect()
}

/// The tracing files, as a listing (diagnostics).
pub fn describe() -> String {
    let mut out = String::new();
    out.push_str("tracing:\n");
    for event in trace::EVENTS.iter() {
        out.push_str(&format!(
            "  {}:{} id={} enabled={}\n",
            event.group,
            event.name,
            event.id,
            event.enabled.load(core::sync::atomic::Ordering::Relaxed)
        ));
    }
    out
}
