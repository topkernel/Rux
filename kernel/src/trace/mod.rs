//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Tracepoints — the event mechanism behind Linux's tracefs.
//!
//! Linux's tracing subsystem has two halves: the EVENT side (a
//! `TRACE_EVENT()` definition per event, a per-event `enable` switch
//! and a per-CPU ring buffer) and the FILE side (tracefs, mounted at
//! /sys/kernel/tracing, with `tracing_on`, `trace`, `available_tracers`
//! and the `events/<group>/<event>/` tree).
//!
//! Rux implements the same two halves:
//!
//! - this module holds the event table, the per-CPU ring buffers and
//!   the trace text renderer;
//! - `fs/tracefs.rs` is the filesystem side.
//!
//! The trace text format is Linux's (fs/trace_output.c):
//!
//! ```text
//!            <task>-<pid>  [001] d..2 12345.678901: sys_enter: nr=1
//! ```
//!
//! an event is recorded only when its own `enable` switch is on AND
//! the global `tracing_on` is on — the same two-level gate Linux uses.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

/// The tracefs files (the filesystem side of the tracing subsystem).
pub mod files;

/// The number of argument slots carried by a trace record.
pub const TRACE_ARGS: usize = 6;

/// How an event renders its arguments (the "print fmt" of the Linux
/// format file).
pub type ArgFormatter = fn(&[u64; TRACE_ARGS]) -> String;

/// One trace event.
pub struct TraceEvent {
    /// The event group (`sched`, `syscalls`, ...).
    pub group: &'static str,
    /// The event name.
    pub name: &'static str,
    /// The event id (Linux assigns unique ids).
    pub id: u16,
    /// The `print fmt` of the event.
    pub print_fmt: &'static str,
    /// The field list, in the Linux "format" file shape.
    pub fields: &'static str,
    /// Per-event enable switch.
    pub enabled: AtomicBool,
    /// The argument renderer.
    pub fmt_args: ArgFormatter,
}

// SAFETY: the event table is read-only after boot except for the
// per-event `enabled` flag, which is an atomic.
unsafe impl Send for TraceEvent {}
unsafe impl Sync for TraceEvent {}

impl TraceEvent {
    /// The full event name, as Linux's tracefs shows it.
    pub fn full_name(&self) -> String {
        format!("{}:{}", self.group, self.name)
    }

    /// The Linux "format" file content for this event.
    pub fn format_file(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("name: {}\n", self.name));
        out.push_str(&format!("ID: {}\n", self.id));
        out.push_str("format:\n");
        // The common fields every Linux trace event carries.
        out.push_str("\tfield:unsigned short common_type;\toffset:0;\tsize:2;\tsigned:0;\n");
        out.push_str("\tfield:unsigned char common_flags;\toffset:2;\tsize:1;\tsigned:0;\n");
        out.push_str("\tfield:unsigned char common_preempt_count;\toffset:3;\tsize:1;\tsigned:0;\n");
        out.push_str("\tfield:int common_pid;\toffset:4;\tsize:4;\tsigned:1;\n");
        // The event's own fields.
        out.push_str(self.fields);
        out.push_str(&format!("\nprint fmt: \"{}\"\n", self.print_fmt));
        out
    }
}

// ============================================================================
// The event table
// ============================================================================

/// syscalls:sys_enter — a syscall entered by a task.
pub static EVENT_SYS_ENTER: TraceEvent = TraceEvent {
    group: "syscalls",
    name: "sys_enter",
    id: 300,
    print_fmt: "\"nr=%d, a0=%lx, a1=%lx, a2=%lx\"",
    fields: "\tfield:long args[6];\toffset:8;\tsize:48;\tsigned:1;\n",
    enabled: AtomicBool::new(false),
    fmt_args: fmt_sys_enter,
};

/// syscalls:sys_exit — a syscall returned.
pub static EVENT_SYS_EXIT: TraceEvent = TraceEvent {
    group: "syscalls",
    name: "sys_exit",
    id: 301,
    print_fmt: "\"nr=%d, ret=%lx\"",
    fields: "\tfield:long ret;\toffset:8;\tsize:8;\tsigned:1;\n",
    enabled: AtomicBool::new(false),
    fmt_args: fmt_sys_exit,
};

/// sched:sched_switch — a context switch.
pub static EVENT_SCHED_SWITCH: TraceEvent = TraceEvent {
    group: "sched",
    name: "sched_switch",
    id: 302,
    print_fmt: "\"prev_comm=%s, prev_pid=%d, next_comm=%s, next_pid=%d\"",
    fields: "\tfield:char prev_comm[16];\toffset:8;\tsize:16;\tsigned:0;\n",
    enabled: AtomicBool::new(false),
    fmt_args: fmt_sched_switch,
};

/// Every event, in id order.
pub static EVENTS: [&TraceEvent; 3] = [
    &EVENT_SYS_ENTER,
    &EVENT_SYS_EXIT,
    &EVENT_SCHED_SWITCH,
];

/// Find an event by group and name.
pub fn find_event(group: &str, name: &str) -> Option<&'static TraceEvent> {
    EVENTS
        .iter()
        .find(|e| e.group == group && e.name == name)
        .copied()
}

// ============================================================================
// The global switch
// ============================================================================

/// The global "tracing_on" switch.
static TRACING_ON: AtomicBool = AtomicBool::new(false);

/// Is tracing enabled globally?
pub fn tracing_on() -> bool {
    TRACING_ON.load(Ordering::Relaxed)
}

/// Turn tracing on or off (the tracefs `tracing_on` file).
pub fn set_tracing_on(on: bool) {
    TRACING_ON.store(on, Ordering::Relaxed);
}

// ============================================================================
// The per-CPU ring buffers
// ============================================================================

/// The capacity of one CPU's ring buffer, in records.
pub const RING_CAPACITY: usize = 4096;

/// One recorded event.
#[derive(Clone, Copy)]
pub struct TraceRecord {
    /// The global sequence number (Linux numbers the records).
    pub seq: u64,
    /// The timestamp in nanoseconds since boot.
    pub ts_ns: u64,
    /// The recording task's pid.
    pub pid: u32,
    /// The recording CPU.
    pub cpu: u16,
    /// The event id.
    pub event_id: u16,
    /// The event's arguments.
    pub args: [u64; TRACE_ARGS],
}

/// One CPU's ring buffer.
pub struct TraceRing {
    /// The records, oldest first.
    pub records: [TraceRecord; RING_CAPACITY],
    /// The next slot to write.
    pub head: AtomicUsize,
    /// How many records are stored.
    pub count: AtomicUsize,
    /// The global sequence counter.
    pub seq: AtomicU64,
}

// SAFETY: the ring's mutable state is behind atomics.
unsafe impl Send for TraceRing {}
unsafe impl Sync for TraceRing {}

impl TraceRing {
    const fn new() -> Self {
        Self {
            records: [TraceRecord {
                seq: 0,
                ts_ns: 0,
                pid: 0,
                cpu: 0,
                event_id: 0,
                args: [0; TRACE_ARGS],
            }; RING_CAPACITY],
            head: AtomicUsize::new(0),
            count: AtomicUsize::new(0),
            seq: AtomicU64::new(0),
        }
    }

    /// Append a record, overwriting the oldest one when full.
    fn push(&self, record: TraceRecord) {
        let idx = self.head.fetch_add(1, Ordering::Relaxed) % RING_CAPACITY;
        // SAFETY: the slot is owned by this push (the head counter is
        // atomic, so two pushes never pick the same slot before the
        // wrap); the record is written before the count is published.
        unsafe {
            let slot = &self.records[idx] as *const TraceRecord as *mut TraceRecord;
            slot.write(record);
        }
        let mut count = self.count.load(Ordering::Relaxed);
        while count < RING_CAPACITY {
            match self.count.compare_exchange(
                count,
                count + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => count = actual,
            }
        }
    }
}

static RINGS: [TraceRing; crate::config::MAX_CPUS] = [const { TraceRing::new() }; crate::config::MAX_CPUS];

/// Record an event on the current CPU.
///
/// Returns false when the event (or the global switch) is off — the
/// check is a single relaxed load, so an unarmed event costs one
/// branch.
pub fn record(event: &TraceEvent, args: [u64; TRACE_ARGS]) -> bool {
    if !TRACING_ON.load(Ordering::Relaxed) || !event.enabled.load(Ordering::Relaxed) {
        return false;
    }
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return false;
    }
    let ring = &RINGS[cpu];
    let seq = ring.seq.fetch_add(1, Ordering::Relaxed);
    ring.push(TraceRecord {
        seq,
        ts_ns: crate::drivers::timer::monotonic_ns_since_boot(),
        pid: crate::sched::get_current_pid(),
        cpu: cpu as u16,
        event_id: event.id,
        args,
    });
    true
}

/// Drop every buffered record (the tracefs `trace` file accepts a
/// write to clear the buffer).
pub fn clear() {
    for ring in RINGS.iter() {
        ring.head.store(0, Ordering::Relaxed);
        ring.count.store(0, Ordering::Relaxed);
    }
}

/// The records of every CPU, in sequence order.
pub fn snapshot() -> Vec<TraceRecord> {
    let mut out: Vec<TraceRecord> = Vec::new();
    for ring in RINGS.iter() {
        let count = ring.count.load(Ordering::Acquire).min(RING_CAPACITY);
        let head = ring.head.load(Ordering::Acquire);
        let start = (head + RING_CAPACITY - count) % RING_CAPACITY;
        for i in 0..count {
            let idx = (start + i) % RING_CAPACITY;
            out.push(ring.records[idx]);
        }
    }
    // The rings are per CPU; the global order is the sequence number.
    out.sort_by_key(|r| r.seq);
    out
}

// ============================================================================
// The text renderer
// ============================================================================

/// Render the buffered records the way Linux's `trace` file does.
///
/// ```text
///            <task>-<pid>  [001] d..2 12345.678901: sys_enter: nr=1, a0=0
/// ```
pub fn render_trace() -> String {
    let mut out = String::new();

    // The header Linux prints above the records.
    out.push_str("# tracer: nop\n");
    out.push_str("#\n");
    let written: usize = RINGS.iter().map(|r| r.seq.load(Ordering::Relaxed) as usize).sum();
    let entries: usize = RINGS.iter().map(|r| r.count.load(Ordering::Relaxed)).sum();
    out.push_str(&format!(
        "# entries-in-buffer/entries-written: {}/{}   #P:0\n",
        entries, written
    ));
    out.push_str("#\n");
    out.push_str("#                                _-----=> irqs-off\n");
    out.push_str("#                               / _----=> need-resched\n");
    out.push_str("#                              | / _---=> hardirq/softirq\n");
    out.push_str("#                              || / _--=> preempt-depth\n");
    out.push_str("#                              ||| / _-=> migrate-disable\n");
    out.push_str("#                              |||| /     delay\n");
    out.push_str("#           TASK-PID    CPU#  |||||  TIMESTAMP  FUNCTION\n");

    for record in snapshot().iter() {
        let event = EVENTS
            .iter()
            .find(|e| e.id == record.event_id)
            .copied();
        let Some(event) = event else { continue };
        let pid = record.pid;
        let comm = current_comm(pid);
        out.push_str(&format!(
            "{:>16}-{:<5} [{:03}] {} {}.{:06}: {}: {}\n",
            comm,
            pid,
            record.cpu,
            trace_flags(),
            record.ts_ns / 1_000_000_000,
            (record.ts_ns % 1_000_000_000) / 1000,
            event.name,
            (event.fmt_args)(&record.args),
        ));
    }

    out
}

/// The four-character flag field Linux prints before the timestamp:
/// irqs-off, need-resched, hardirq/softirq and the preempt depth.
fn trace_flags() -> String {
    // Linux's four-character flag field: irqs-off, need-resched,
    // hardirq/softirq and the preempt depth. Rux has no "irqs off"
    // query, so the first column reports whether the CPU is in an
    // interrupt context at all, and the second reports which kind.
    let mut out = String::with_capacity(4);
    out.push(if crate::interrupt::preempt::in_interrupt() {
        'd'
    } else {
        '.'
    });
    out.push(if crate::interrupt::preempt::in_irq() {
        'h'
    } else if crate::interrupt::preempt::in_softirq() {
        's'
    } else {
        '.'
    });
    out.push('.');
    let depth = crate::interrupt::preempt::preempt_count();
    out.push(if depth > 0 {
        char::from(b'0' + (depth as u8).min(9))
    } else {
        '.'
    });
    out
}

/// The command name of a task (Linux prints the comm).
fn current_comm(pid: u32) -> String {
    if let Some(task) = crate::process::find_task_by_pid(pid) {
        let comm = task.comm();
        let end = comm.iter().position(|&b| b == 0).unwrap_or(comm.len());
        return String::from_utf8_lossy(&comm[..end]).into_owned();
    }
    "<none>".to_string()
}

// ============================================================================
// The argument formatters
// ============================================================================

fn fmt_sys_enter(args: &[u64; TRACE_ARGS]) -> String {
    format!(
        "nr={}, a0={:#x}, a1={:#x}, a2={:#x}",
        args[0], args[1], args[2], args[3]
    )
}

fn fmt_sys_exit(args: &[u64; TRACE_ARGS]) -> String {
    format!("nr={}, ret={:#x}", args[0], args[1])
}

fn fmt_sched_switch(args: &[u64; TRACE_ARGS]) -> String {
    let comm = |value: u64| -> String {
        let bytes = value.to_le_bytes();
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(16);
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    };
    format!(
        "prev_comm={}, prev_pid={}, next_comm={}, next_pid={}",
        comm(args[0]),
        args[1],
        comm(args[2]),
        args[3]
    )
}
