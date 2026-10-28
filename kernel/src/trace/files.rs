//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! The tracefs FILES — the filesystem side of the tracing subsystem.
//!
//! The standard tracefs carries the tracing control surface. The file set
//! below is the subset Rux implements, each file with the standard name,
//! mode and byte format (captured from this host's live
//! /sys/kernel/tracing on this host):
//!
//! ```text
//! /sys/kernel/tracing/           (Rux: /sys/kernel/debug/tracing)
//! ├── tracing_on          0640  the master switch ("1\n"/"0\n")
//! ├── trace               0640  the buffered records (write clears)
//! ├── trace_pipe          0440  the records, consumed on read, no header
//! ├── available_tracers   0440  "nop\n"
//! ├── current_tracer      0640  "nop\n" (writing anything else: EINVAL)
//! ├── available_events    0440  "group:name\n" per event
//! ├── set_event           0640  the enabled events; write to (dis)arm
//! ├── buffer_size_kb      0640  the per-CPU ring size in KiB
//! └── events/
//!     ├── enable          0640  all events (1/0/X, X = mixed)
//!     └── <group>/
//!         ├── enable      0640  the group's events (1/0/X)
//!         ├── filter     0640  the global-filter note (the standard text)
//!         └── <event>/
//!             ├── enable  0640  1/0
//!             ├── filter  0640  "none" (no filter engine)
//!             ├── id      0440  the numeric event id
//!             └── format  0440  the TRACE_EVENT field description
//! ```
//!
//! The X convention is the standard one: a group (or the whole tree) reads
//! "1" when every event is enabled, "0" when none is, "X" when the
//! set is mixed.

// Framekernel layer policy (docs/architecture/framekernel-layers.md):
// this module must stay Safe Rust.
#![forbid(unsafe_code)]

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::fs::debugfs;
use crate::trace;

/// The directory the tracing files live in (inside debugfs, which is
/// mounted at /sys/kernel/debug — the path the kernel documentation has
/// historically used for the same file set).
const TRACING_DIR: &str = "/tracing";

/// The group-level filter file text (standard wording) (events/<group>/filter).
const GROUP_FILTER_TEXT: &str = "### global filter ###\n# Use this to set filters for multiple events.\n# Only events with the given fields will be affected.\n# If no events are modified, an error message will be displayed here\n";

/// Parse a boolean knob the standard way: any number is accepted,
/// zero is off, everything else is on.
///
/// A whitespace-only buffer is a NO-OP success: the guest shell sends
/// the value and the trailing newline as separate write() calls, and
/// the newline-only write must not fail the shell's redirection.
fn parse_bool(buf: &[u8]) -> Option<Option<bool>> {
    let text = core::str::from_utf8(buf).ok()?.trim();
    if text.is_empty() {
        return Some(None);
    }
    match text.parse::<u64>() {
        Ok(v) => Some(Some(v != 0)),
        Err(_) => None,
    }
}

/// A boolean write handler body: `None` from parse_bool is a syntax
/// error (EINVAL); `Some(None)` is the no-op newline write.
fn bool_write(buf: &[u8], apply: impl Fn(bool)) -> i32 {
    match parse_bool(buf) {
        Some(Some(on)) => {
            apply(on);
            0
        }
        Some(None) => 0,
        None => crate::errno::Errno::InvalidArgument.as_neg_i32(),
    }
}

/// The 1/0/X reading of a set of enable switches.
fn set_state(on: usize, total: usize) -> char {
    if total == 0 || on == 0 {
        '0'
    } else if on == total {
        '1'
    } else {
        'X'
    }
}

/// The distinct groups, in name order.
fn groups() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = trace::EVENTS.iter().map(|e| e.group).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Publish the tracing files.
pub fn register() {
    if !debugfs::create_dir(TRACING_DIR) {
        return;
    }

    // --- the master switch ------------------------------------------
    debugfs::create_file(
        &format!("{}/tracing_on", TRACING_DIR),
        0o640,
        || {
            let mut out = alloc::vec![b'0'];
            if trace::tracing_on() {
                out[0] = b'1';
            }
            out.push(b'\n');
            out
        },
        Some(|buf: &[u8]| bool_write(buf, |on| trace::set_tracing_on(on))),
    );

    // --- the collected records ---------------------------------------
    debugfs::create_file(
        &format!("{}/trace", TRACING_DIR),
        0o640,
        || trace::render_trace().into_bytes(),
        // Writing to the trace file clears the buffer (standard behavior).
        Some(|_buf: &[u8]| {
            trace::clear();
            0
        }),
    );

    // --- the consuming reader ----------------------------------------
    debugfs::create_file(
        &format!("{}/trace_pipe", TRACING_DIR),
        0o440,
        || trace::render_pipe().into_bytes(),
        None::<fn(&[u8]) -> i32>,
    );

    // --- the tracers --------------------------------------------------
    // Rux has no tracer of its own: the events are the whole facility,
    // which is what the "nop" tracer means — it records nothing by
    // itself, the tracepoints do.
    debugfs::create_file(
        &format!("{}/available_tracers", TRACING_DIR),
        0o440,
        || b"nop\n".to_vec(),
        None::<fn(&[u8]) -> i32>,
    );
    debugfs::create_file(
        &format!("{}/current_tracer", TRACING_DIR),
        0o640,
        || b"nop\n".to_vec(),
        Some(|buf: &[u8]| {
            let text = core::str::from_utf8(buf).unwrap_or("").trim();
            if text.is_empty() {
                // The shell's split trailing-newline write: no-op.
                0
            } else if text == "nop" {
                0
            } else {
                // An unknown tracer is refused (EINVAL).
                crate::errno::Errno::InvalidArgument.as_neg_i32()
            }
        }),
    );

    // --- the event catalog --------------------------------------------
    debugfs::create_file(
        &format!("{}/available_events", TRACING_DIR),
        0o440,
        || {
            let mut names: Vec<String> = trace::EVENTS
                .iter()
                .map(|e| format!("{}:{}", e.group, e.name))
                .collect();
            names.sort();
            let mut out = String::new();
            for name in names {
                out.push_str(&name);
                out.push('\n');
            }
            out.into_bytes()
        },
        None::<fn(&[u8]) -> i32>,
    );
    // set_event: read lists the armed events; write (dis)arms by name.
    debugfs::create_file(
        &format!("{}/set_event", TRACING_DIR),
        0o640,
        || {
            let mut out = String::new();
            let mut names: Vec<String> = trace::EVENTS
                .iter()
                .filter(|e| {
                    e.enabled
                        .load(core::sync::atomic::Ordering::Relaxed)
                })
                .map(|e| format!("{}:{}", e.group, e.name))
                .collect();
            names.sort();
            for name in names {
                out.push_str(&name);
                out.push('\n');
            }
            out.into_bytes()
        },
        Some(|buf: &[u8]| set_event_write(buf)),
    );

    // --- the ring size -------------------------------------------------
    debugfs::create_file(
        &format!("{}/buffer_size_kb", TRACING_DIR),
        0o640,
        || {
            let kb =
                trace::RING_CAPACITY * core::mem::size_of::<trace::TraceRecord>() / 1024;
            alloc::format!("{}\n", kb).into_bytes()
        },
        // The rings are statically allocated; there is nothing to
        // resize, so every write is refused.
        Some(|_buf: &[u8]| crate::errno::Errno::InvalidArgument.as_neg_i32()),
    );

    // --- the event tree ------------------------------------------------
    let events_dir = format!("{}/events", TRACING_DIR);
    for group in groups() {
        let group_dir = format!("{}/{}", events_dir, group);
        for event_ref in trace::EVENTS.iter().filter(|e| e.group == group) {
            // A 'static reference, so the closures below can own it.
            let event: &'static trace::TraceEvent = event_ref;
            let event_dir = format!("{}/{}", group_dir, event.name);

            // The per-event switch.
            debugfs::create_file(
                &format!("{}/enable", event_dir),
                0o640,
                || {
                    let mut out = alloc::vec![b'0'];
                    if event
                        .enabled
                        .load(core::sync::atomic::Ordering::Relaxed)
                    {
                        out[0] = b'1';
                    }
                    out.push(b'\n');
                    out
                },
                Some(move |buf: &[u8]| {
                    bool_write(buf, |on| {
                        event
                            .enabled
                            .store(on, core::sync::atomic::Ordering::Relaxed)
                    })
                }),
            );

            // The event id.
            debugfs::create_file(
                &format!("{}/id", event_dir),
                0o440,
                || alloc::format!("{}\n", event.id).into_bytes(),
                None::<fn(&[u8]) -> i32>,
            );

            // The format description.
            debugfs::create_file(
                &format!("{}/format", event_dir),
                0o440,
                || event.format_file().into_bytes(),
                None::<fn(&[u8]) -> i32>,
            );

            // The event filter — no filter engine, so like a standard
            // event with no filter set: reads "none", accepts nothing.
            debugfs::create_file(
                &format!("{}/filter", event_dir),
                0o640,
                || b"none\n".to_vec(),
                Some(|_buf: &[u8]| crate::errno::Errno::InvalidArgument.as_neg_i32()),
            );
        }

        // The group switch: all the events of the group (1/0/X). The
        // closures capture only the 'static group name and re-filter
        // the event table.
        debugfs::create_file(
            &format!("{}/enable", group_dir),
            0o640,
            move || {
                let on = trace::EVENTS
                    .iter()
                    .filter(|e| {
                        e.group == group
                            && e.enabled
                                .load(core::sync::atomic::Ordering::Relaxed)
                    })
                    .count();
                let total = trace::EVENTS.iter().filter(|e| e.group == group).count();
                let mut out = alloc::vec![set_state(on, total) as u8];
                out.push(b'\n');
                out
            },
            Some(move |buf: &[u8]| {
                bool_write(buf, |on| {
                    for e in trace::EVENTS.iter().filter(|e| e.group == group) {
                        e.enabled
                            .store(on, core::sync::atomic::Ordering::Relaxed);
                    }
                })
            }),
        );

        // The group filter — the standard global-filter note.
        debugfs::create_file(
            &format!("{}/filter", group_dir),
            0o640,
            || GROUP_FILTER_TEXT.as_bytes().to_vec(),
            Some(|_buf: &[u8]| crate::errno::Errno::InvalidArgument.as_neg_i32()),
        );
    }

    // The whole-tree switch (events/enable): 1/0/X across every event.
    debugfs::create_file(
        &format!("{}/enable", events_dir),
        0o640,
        || {
            let on = trace::EVENTS
                .iter()
                .filter(|e| {
                    e.enabled
                        .load(core::sync::atomic::Ordering::Relaxed)
                })
                .count();
            let mut out = alloc::vec![set_state(on, trace::EVENTS.len()) as u8];
            out.push(b'\n');
            out
        },
        Some(|buf: &[u8]| {
            bool_write(buf, |on| {
                for e in trace::EVENTS.iter() {
                    e.enabled
                        .store(on, core::sync::atomic::Ordering::Relaxed);
                }
            })
        }),
    );

    // --- trace_marker: the write-only marker file ----------------------
    // Mode 0220 and write-only, exactly the reference file. Markers are
    // gated by tracing_on alone (no event switch); the write body
    // records the text as a tracing_mark_write event. The read closure
    // never runs through the mode gate — the reference answers a read
    // of the write-only file with EINVAL; the mode refusal here is the
    // same refusal at a different errno.
    debugfs::create_file(
        &format!("{}/trace_marker", TRACING_DIR),
        0o220,
        || Vec::new(),
        Some(|buf: &[u8]| {
            trace::record_marker(buf);
            0
        }),
    );

    // --- the per-CPU views ---------------------------------------------
    // per_cpu/cpuN/{trace,trace_pipe,stats,buffer_size_kb}: the
    // reference dir also carries the snapshot and raw views; those
    // need a second buffer / a binary record format this ring does not
    // have, so they are omitted rather than faked (the plan doc's
    // do-not-invent rule).
    for cpu in 0..crate::config::MAX_CPUS {
        let cpu_dir = format!("{}/per_cpu/cpu{}", TRACING_DIR, cpu);
        debugfs::create_file(
            &format!("{}/trace", cpu_dir),
            0o640,
            move || trace::render_trace_cpu(cpu).into_bytes(),
            Some(move |_buf: &[u8]| {
                // Writing the per-CPU trace file clears that CPU's
                // ring, the global file's convention.
                trace::clear_cpu(cpu);
                0
            }),
        );
        debugfs::create_file(
            &format!("{}/trace_pipe", cpu_dir),
            0o440,
            move || trace::render_pipe_cpu(cpu).into_bytes(),
            None::<fn(&[u8]) -> i32>,
        );
        debugfs::create_file(
            &format!("{}/stats", cpu_dir),
            0o440,
            move || trace::cpu_stats(cpu).into_bytes(),
            None::<fn(&[u8]) -> i32>,
        );
        debugfs::create_file(
            &format!("{}/buffer_size_kb", cpu_dir),
            0o640,
            || {
                let kb = trace::RING_CAPACITY
                    * core::mem::size_of::<trace::TraceRecord>()
                    / 1024;
                alloc::format!("{}\n", kb).into_bytes()
            },
            None::<fn(&[u8]) -> i32>,
        );
    }
}

/// The set_event write handler: comma- or space-separated tokens,
/// "group:name" (or ":name" for any group, or "all"/"none"), an
/// optional '!' prefix disarms instead of arming — the standard grammar.
fn set_event_write(buf: &[u8]) -> i32 {
    let text = match core::str::from_utf8(buf) {
        Ok(t) => t.trim(),
        Err(_) => return crate::errno::Errno::InvalidArgument.as_neg_i32(),
    };
    for token in text.split(|c| c == ',' || c == ' ') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        let (disarm, name) = match token.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, token),
        };
        let enable = |e: &trace::TraceEvent| {
            e.enabled
                .store(!disarm, core::sync::atomic::Ordering::Relaxed);
        };
        match name {
            "all" => {
                for e in trace::EVENTS.iter() {
                    enable(e);
                }
            }
            "none" => {
                for e in trace::EVENTS.iter() {
                    e.enabled
                        .store(false, core::sync::atomic::Ordering::Relaxed);
                }
            }
            _ => {
                let (group, event) = match name.split_once(':') {
                    Some((g, n)) => (g, n),
                    None => return crate::errno::Errno::InvalidArgument.as_neg_i32(),
                };
                // ":name" (empty group) arms every group's event of
                // that name (any group).
                let matches: Vec<&trace::TraceEvent> = trace::EVENTS
                    .iter()
                    .filter(|e| (group.is_empty() || e.group == group) && e.name == event)
                    .copied()
                    .collect();
                if matches.is_empty() {
                    return crate::errno::Errno::InvalidArgument.as_neg_i32();
                }
                for e in matches {
                    enable(e);
                }
            }
        }
    }
    0
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
