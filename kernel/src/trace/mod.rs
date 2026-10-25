//! MIT License
//!
//! Copyright (c) 2026 Fei Wang
//!
//! Tracepoints — the event mechanism behind the standard tracefs.
//!
//! the reference tracing subsystem has two halves: the EVENT side (a
//! `TRACE_EVENT()` definition per event, a per-event `enable` switch
//! and a per-CPU ring buffer) and the FILE side (tracefs, mounted at
//! /sys/kernel/tracing, with `tracing_on`, `trace`, `trace_pipe`,
//! `available_events`, `set_event` and the `events/<group>/<event>/`
//! tree).
//!
//! Rux implements the same two halves:
//!
//! - this module holds the event table, the per-CPU ring buffers and
//!   the trace text renderer;
//! - `trace::files` is the tracefs side (published under
//!   /sys/kernel/debug/tracing, the path the reference host's documentation has
//!   historically used).
//!
//! The trace text format is the reference host's (fs/trace_output.c), captured
//! byte-for-byte from this host's live /sys/kernel/tracing:
//!
//! ```text
//! # tracer: nop
//! #
//! # entries-in-buffer/entries-written: 9777/9777   #P:32
//! #
//! #                                _-----=> irqs-off/BH-disabled
//! #                               / _----=> need-resched
//! #                              | / _---=> hardirq/softirq
//! #                              || / _--=> preempt-depth
//! #                              ||| / _-=> migrate-disable
//! #                              |||| /     delay
//! #           TASK-PID     CPU#  |||||  TIMESTAMP  FUNCTION
//! #              | |         |   |||||     |         |
//!           <idle>-0       [010] d..2. 125097.209708: sched_switch: prev_comm=swapper/10 ...
//! ```
//!
//! A record line is `%16s-%-7d [%03d] %c%c%c%c%c %5lu.%06lu: %s: %s`
//! (16-wide right-aligned comm, `-`, 7-wide left-aligned pid, `[#CPU]`,
//! the five flag characters, 5.6-digit seconds-since-boot timestamp).
//!
//! An event is recorded only when its own `enable` switch is on AND
//! the global `tracing_on` is on — the same two-level gate the
//! reference host uses. Both checks are relaxed loads, so a call site's disabled
//! path is one branch after the two loads (see `TraceEvent::armed`);
//! call sites must marshal their arguments only after `armed()`
//! returns true.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

/// The tracefs files (the filesystem side of the tracing subsystem).
pub mod files;

/// The number of argument slots carried by a trace record.
///
/// Ten: `sched_switch` carries two 16-byte comms (two slots each),
/// two pids, two prios and one state — nine in total.
pub const TRACE_ARGS: usize = 10;

/// TASK_COMM_LEN (16, the standard task-comm size).
pub const COMM_LEN: usize = 16;

/// How an event renders its arguments (the "print fmt" of the standard
/// format file).
pub type ArgFormatter = fn(&[u64; TRACE_ARGS]) -> String;

/// One trace event.
pub struct TraceEvent {
    /// The event group (`sched`, `raw_syscalls`, ...).
    pub group: &'static str,
    /// The event name.
    pub name: &'static str,
    /// The event id (a unique per-event number).
    pub id: u16,
    /// The `print fmt` of the event.
    pub print_fmt: &'static str,
    /// The field list, in the standard "format" file shape (each line
    /// tab-prefixed, `field:...;\toffset:N;\tsize:N;\tsigned:N;\n`).
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
    /// Is this event armed (its own switch AND the global switch)?
    ///
    /// Two relaxed loads; call sites guard their argument marshalling
    /// behind this so the disabled path marshals nothing.
    #[inline]
    pub fn armed(&self) -> bool {
        TRACING_ON.load(Ordering::Relaxed) && self.enabled.load(Ordering::Relaxed)
    }

    /// The full event name, as the reference host's available_events shows it.
    pub fn full_name(&self) -> String {
        format!("{}:{}", self.group, self.name)
    }

    /// The "format" file content for this event (the common
    /// fields, a blank line, the event's fields, a blank line, the
    /// print fmt — the live file's exact shape).
    pub fn format_file(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("name: {}\n", self.name));
        out.push_str(&format!("ID: {}\n", self.id));
        out.push_str("format:\n");
        // The common fields every trace event carries.
        out.push_str("\tfield:unsigned short common_type;\toffset:0;\tsize:2;\tsigned:0;\n");
        out.push_str("\tfield:unsigned char common_flags;\toffset:2;\tsize:1;\tsigned:0;\n");
        out.push_str("\tfield:unsigned char common_preempt_count;\toffset:3;\tsize:1;\tsigned:0;\n");
        out.push_str("\tfield:int common_pid;\toffset:4;\tsize:4;\tsigned:1;\n");
        out.push('\n');
        // The event's own fields.
        out.push_str(self.fields);
        out.push('\n');
        out.push_str(&format!("print fmt: {}\n", self.print_fmt));
        out
    }
}

// ============================================================================
// The event table
// ============================================================================

/// raw_syscalls:sys_enter — every syscall entry, with the raw (native)
/// number and the six register arguments. the reference host's
/// events/raw_syscalls/sys_enter, format captured from the live file.
pub static EVENT_SYS_ENTER: TraceEvent = TraceEvent {
    group: "raw_syscalls",
    name: "sys_enter",
    id: 1,
    print_fmt: "\"NR %ld (%lx, %lx, %lx, %lx, %lx, %lx)\", REC->id, REC->args[0], REC->args[1], REC->args[2], REC->args[3], REC->args[4], REC->args[5]",
    fields: "\tfield:long id;\toffset:8;\tsize:8;\tsigned:1;\n\
             \tfield:unsigned long args[6];\toffset:16;\tsize:48;\tsigned:0;\n",
    enabled: AtomicBool::new(false),
    fmt_args: fmt_sys_enter,
};

/// raw_syscalls:sys_exit — every syscall return, with the raw number
/// and the return value. the reference host's events/raw_syscalls/sys_exit.
pub static EVENT_SYS_EXIT: TraceEvent = TraceEvent {
    group: "raw_syscalls",
    name: "sys_exit",
    id: 2,
    print_fmt: "\"NR %ld = %ld\", REC->id, REC->ret",
    fields: "\tfield:long id;\toffset:8;\tsize:8;\tsigned:1;\n\
             \tfield:long ret;\toffset:16;\tsize:8;\tsigned:1;\n",
    enabled: AtomicBool::new(false),
    fmt_args: fmt_sys_exit,
};

/// sched:sched_switch — a context switch. the reference host's
/// events/sched/sched_switch; the print fmt is the live file's blob
/// verbatim (the __print_flags expression for prev_state).
pub static EVENT_SCHED_SWITCH: TraceEvent = TraceEvent {
    group: "sched",
    name: "sched_switch",
    id: 3,
    print_fmt: "\"prev_comm=%s prev_pid=%d prev_prio=%d prev_state=%s%s ==> next_comm=%s next_pid=%d next_prio=%d\", REC->prev_comm, REC->prev_pid, REC->prev_prio, (REC->prev_state & ((((0x00000000 | 0x00000001 | 0x00000002 | 0x00000004 | 0x00000008 | 0x00000010 | 0x00000020 | 0x00000040) + 1) << 1) - 1)) ? __print_flags(REC->prev_state & ((((0x00000000 | 0x00000001 | 0x00000002 | 0x00000004 | 0x00000008 | 0x00000010 | 0x00000020 | 0x00000040) + 1) << 1) - 1), \"|\", { 0x00000001, \"S\" }, { 0x00000002, \"D\" }, { 0x00000004, \"T\" }, { 0x00000008, \"t\" }, { 0x00000010, \"X\" }, { 0x00000020, \"Z\" }, { 0x00000040, \"P\" }, { 0x00000080, \"I\" }) : \"R\", REC->prev_state & (((0x00000000 | 0x00000001 | 0x00000002 | 0x00000004 | 0x00000008 | 0x00000010 | 0x00000020 | 0x00000040) + 1) << 1) ? \"+\" : \"\", REC->next_comm, REC->next_pid, REC->next_prio",
    fields: "\tfield:char prev_comm[16];\toffset:8;\tsize:16;\tsigned:0;\n\
             \tfield:pid_t prev_pid;\toffset:24;\tsize:4;\tsigned:1;\n\
             \tfield:int prev_prio;\toffset:28;\tsize:4;\tsigned:1;\n\
             \tfield:long prev_state;\toffset:32;\tsize:8;\tsigned:1;\n\
             \tfield:char next_comm[16];\toffset:40;\tsize:16;\tsigned:0;\n\
             \tfield:pid_t next_pid;\toffset:56;\tsize:4;\tsigned:1;\n\
             \tfield:int next_prio;\toffset:60;\tsize:4;\tsigned:1;\n",
    enabled: AtomicBool::new(false),
    fmt_args: fmt_sched_switch,
};

/// Every event, in id order.
pub static EVENTS: [&TraceEvent; 10] = [
    &EVENT_SYS_ENTER,
    &EVENT_SYS_EXIT,
    &EVENT_SCHED_SWITCH,
    &EVENT_SCHED_WAKING,
    &EVENT_SCHED_WAKEUP,
    &EVENT_SCHED_WAKEUP_NEW,
    &EVENT_SCHED_PROCESS_FORK,
    &EVENT_SCHED_PROCESS_EXIT,
    &EVENT_PAGE_FAULT_USER,
    &EVENT_PAGE_FAULT_KERNEL,
];

/// sched:sched_waking — a task is about to be woken (emitted before the
/// enqueue attempt). The reference host's events/sched/sched_waking;
/// the fields and print fmt are the live file's, identical to
/// sched_wakeup's.
pub static EVENT_SCHED_WAKING: TraceEvent = TraceEvent {
    group: "sched",
    name: "sched_waking",
    id: 4,
    print_fmt: "\"comm=%s pid=%d prio=%d target_cpu=%03d\", REC->comm, REC->pid, REC->prio, REC->target_cpu",
    fields: "\tfield:char comm[16];\toffset:8;\tsize:16;\tsigned:0;\n\
             \tfield:pid_t pid;\toffset:24;\tsize:4;\tsigned:1;\n\
             \tfield:int prio;\toffset:28;\tsize:4;\tsigned:1;\n\
             \tfield:int target_cpu;\toffset:32;\tsize:4;\tsigned:1;\n",
    enabled: AtomicBool::new(false),
    fmt_args: fmt_sched_wakeup,
};

/// sched:sched_wakeup — a task left the wait queue and was enqueued
/// (recorded after the enqueue succeeded). The reference host's
/// events/sched/sched_wakeup.
pub static EVENT_SCHED_WAKEUP: TraceEvent = TraceEvent {
    group: "sched",
    name: "sched_wakeup",
    id: 5,
    print_fmt: "\"comm=%s pid=%d prio=%d target_cpu=%03d\", REC->comm, REC->pid, REC->prio, REC->target_cpu",
    fields: "\tfield:char comm[16];\toffset:8;\tsize:16;\tsigned:0;\n\
             \tfield:pid_t pid;\toffset:24;\tsize:4;\tsigned:1;\n\
             \tfield:int prio;\toffset:28;\tsize:4;\tsigned:1;\n\
             \tfield:int target_cpu;\toffset:32;\tsize:4;\tsigned:1;\n",
    enabled: AtomicBool::new(false),
    fmt_args: fmt_sched_wakeup,
};

/// sched:sched_wakeup_new — the FIRST enqueue of a freshly built task
/// (the fork path's enqueue). The reference host's
/// events/sched/sched_wakeup_new; fields identical to sched_wakeup's.
pub static EVENT_SCHED_WAKEUP_NEW: TraceEvent = TraceEvent {
    group: "sched",
    name: "sched_wakeup_new",
    id: 6,
    print_fmt: "\"comm=%s pid=%d prio=%d target_cpu=%03d\", REC->comm, REC->pid, REC->prio, REC->target_cpu",
    fields: "\tfield:char comm[16];\toffset:8;\tsize:16;\tsigned:0;\n\
             \tfield:pid_t pid;\toffset:24;\tsize:4;\tsigned:1;\n\
             \tfield:int prio;\toffset:28;\tsize:4;\tsigned:1;\n\
             \tfield:int target_cpu;\toffset:32;\tsize:4;\tsigned:1;\n",
    enabled: AtomicBool::new(false),
    fmt_args: fmt_sched_wakeup,
};

/// sched:sched_process_fork — a child task was built. The reference
/// host's events/sched/sched_process_fork; the __data_loc char[]
/// fields are the live file's (a dynamic string carries its blob
/// offset in the field word; Rux records the comms inline instead and
/// renders them the same way).
pub static EVENT_SCHED_PROCESS_FORK: TraceEvent = TraceEvent {
    group: "sched",
    name: "sched_process_fork",
    id: 7,
    print_fmt: "\"comm=%s pid=%d child_comm=%s child_pid=%d\", __get_str(parent_comm), REC->parent_pid, __get_str(child_comm), REC->child_pid",
    fields: "\tfield:__data_loc char[] parent_comm;\toffset:8;\tsize:4;\tsigned:0;\n\
             \tfield:pid_t parent_pid;\toffset:12;\tsize:4;\tsigned:1;\n\
             \tfield:__data_loc char[] child_comm;\toffset:16;\tsize:4;\tsigned:0;\n\
             \tfield:pid_t child_pid;\toffset:20;\tsize:4;\tsigned:1;\n",
    enabled: AtomicBool::new(false),
    fmt_args: fmt_sched_process_fork,
};

/// sched:sched_process_exit — a task entered do_exit. The reference
/// host's events/sched/sched_process_exit.
pub static EVENT_SCHED_PROCESS_EXIT: TraceEvent = TraceEvent {
    group: "sched",
    name: "sched_process_exit",
    id: 8,
    print_fmt: "\"comm=%s pid=%d prio=%d group_dead=%s\", REC->comm, REC->pid, REC->prio, REC->group_dead ? \"true\" : \"false\"",
    fields: "\tfield:char comm[16];\toffset:8;\tsize:16;\tsigned:0;\n\
             \tfield:pid_t pid;\toffset:24;\tsize:4;\tsigned:1;\n\
             \tfield:int prio;\toffset:28;\tsize:4;\tsigned:1;\n\
             \tfield:bool group_dead;\toffset:32;\tsize:1;\tsigned:0;\n",
    enabled: AtomicBool::new(false),
    fmt_args: fmt_sched_process_exit,
};

/// exceptions:page_fault_user — a page fault taken from user mode. The
/// reference host does not carry the exceptions group (it is the
/// architecture fault-trace family); the field set and print fmt are
/// the generic definition's (address/ip/error_code, %ps symbolic
/// rendering). On riscv64 the error_code carries the access-type bits
/// (read=0x1 write=0x2 exec=0x4) rather than the x86 error-word, and
/// ip is the faulting epc.
pub static EVENT_PAGE_FAULT_USER: TraceEvent = TraceEvent {
    group: "exceptions",
    name: "page_fault_user",
    id: 9,
    print_fmt: "\"address=%ps ip=%ps error_code=0x%lx\", (void *)REC->address, (void *)REC->ip, REC->error_code",
    fields: "\tfield:unsigned long address;\toffset:8;\tsize:8;\tsigned:0;\n\
             \tfield:unsigned long ip;\toffset:16;\tsize:8;\tsigned:0;\n\
             \tfield:unsigned long error_code;\toffset:24;\tsize:8;\tsigned:0;\n",
    enabled: AtomicBool::new(false),
    fmt_args: fmt_page_fault,
};

/// exceptions:page_fault_kernel — a page fault taken from kernel mode
/// (a vmalloc-style fault or a kernel bug). Same shape as
/// page_fault_user's.
pub static EVENT_PAGE_FAULT_KERNEL: TraceEvent = TraceEvent {
    group: "exceptions",
    name: "page_fault_kernel",
    id: 10,
    print_fmt: "\"address=%ps ip=%ps error_code=0x%lx\", (void *)REC->address, (void *)REC->ip, REC->error_code",
    fields: "\tfield:unsigned long address;\toffset:8;\tsize:8;\tsigned:0;\n\
             \tfield:unsigned long ip;\toffset:16;\tsize:8;\tsigned:0;\n\
             \tfield:unsigned long error_code;\toffset:24;\tsize:8;\tsigned:0;\n",
    enabled: AtomicBool::new(false),
    fmt_args: fmt_page_fault,
};

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
///
/// 1024 records x 128 bytes = 128 KiB per CPU (512 KiB at 4 CPUs):
/// the .bss footprint stays far below the point the perf lane has
/// measured as noticeable, and the buffer still holds thousands of
/// events between reader visits.
pub const RING_CAPACITY: usize = 1024;

/// One recorded event.
#[derive(Clone, Copy)]
pub struct TraceRecord {
    /// The global sequence number (a single counter, so records from
    /// every CPU share one total order).
    pub seq: u64,
    /// The timestamp in nanoseconds since boot.
    pub ts_ns: u64,
    /// The recording task's pid.
    pub pid: u32,
    /// The recording CPU.
    pub cpu: u16,
    /// The event id.
    pub event_id: u16,
    /// The five standard flag characters, pre-rendered at record time
    /// (irqs-off, need-resched, hardirq/softirq, preempt-depth,
    /// migrate-disable).
    pub flags: [u8; 5],
    /// The recording task's comm, copied at record time (the reference host saves
    /// comms at record time too — a pid looked up later may be gone
    /// or reused).
    pub comm: [u8; COMM_LEN],
    /// The event's arguments.
    pub args: [u64; TRACE_ARGS],
}

/// One CPU's ring buffer.
pub struct TraceRing {
    /// The records.
    pub records: [TraceRecord; RING_CAPACITY],
    /// The next slot to write.
    pub head: AtomicUsize,
    /// How many records are stored.
    pub count: AtomicUsize,
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
                flags: [b'.', b'.', b'.', b'.', b'.'],
                comm: [0; COMM_LEN],
                args: [0; TRACE_ARGS],
            }; RING_CAPACITY],
            head: AtomicUsize::new(0),
            count: AtomicUsize::new(0),
        }
    }

    /// Append a record, overwriting the oldest one when full.
    fn push(&self, record: TraceRecord) {
        let idx = self.head.fetch_add(1, Ordering::Relaxed) % RING_CAPACITY;
        // SAFETY: the head counter hands out distinct slots to
        // concurrent writers, so this slot is owned by this push; the
        // record is written before the count is published. A reader
        // may race with the write and observe a torn record — an
        // acceptable debug-facility trade (the reference kernel excludes readers
        // with per-CPU buffer locks; the cost is not warranted here).
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

static RINGS: [TraceRing; crate::config::MAX_CPUS] =
    [const { TraceRing::new() }; crate::config::MAX_CPUS];

/// The global sequence counter (one counter for every CPU, so the
/// reader can merge the rings into the true recording order).
static GLOBAL_SEQ: AtomicU64 = AtomicU64::new(0);

/// The total number of records ever written.
pub fn records_written() -> u64 {
    GLOBAL_SEQ.load(Ordering::Relaxed)
}

/// The number of records currently buffered.
pub fn records_buffered() -> usize {
    RINGS
        .iter()
        .map(|r| r.count.load(Ordering::Relaxed).min(RING_CAPACITY))
        .sum()
}

/// Render the five flag characters (render_flags below).
fn render_flags() -> [u8; 5] {
    let mut out = [b'.'; 5];
    // 1: irqs-off — 'd' when the interrupt flag is clear, exactly the
    // condition the reference reports (a sched_switch line reads d..1. or
    // d..2. because __schedule runs with irqs off; a syscall line
    // reads ...).
    if !crate::arch::cpu::get_interrupts_state() {
        out[0] = b'd';
    }
    // 2: need-resched — cleared before the switch is recorded;
    // Rux clears it the same way at __schedule entry, so '.'.
    // 3: hardirq/softirq.
    if crate::interrupt::preempt::in_irq() {
        out[2] = b'h';
    } else if crate::interrupt::preempt::in_softirq() {
        out[2] = b's';
    }
    // 4: preempt-depth (the low nibble of the count, as a digit).
    let depth = crate::interrupt::preempt::preempt_count() & 0xf;
    if (1..=9).contains(&depth) {
        out[3] = b'0' + depth as u8;
    }
    // 5: migrate-disable — Rux does not migrate-disable; always '.'.
    out
}

/// Record an event on the current CPU.
///
/// `record` re-checks the two switches (a call site that already
/// called `armed()` pays nothing extra the second time — both are
/// relaxed loads of words now hot in cache).
pub fn record(event: &TraceEvent, args: [u64; TRACE_ARGS]) -> bool {
    if !event.armed() {
        return false;
    }
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return false;
    }
    // Copy the comm at record time (pid lookup later could hit a
    // recycled pid). The idle task's comm is empty; the reference names it
    // swapper/N in the sched_switch arguments and <idle> in the
    // TASK-PID column — the renderer handles the column name.
    let mut comm = [0u8; COMM_LEN];
    let mut pid = 0u32;
    if let Some(cur) = crate::sched::current() {
        // SAFETY: cur is the running task.
        unsafe {
            comm = *(*cur).comm();
            pid = (*cur).pid() as u32;
        }
    }
    let ring = &RINGS[cpu];
    let seq = GLOBAL_SEQ.fetch_add(1, Ordering::Relaxed);
    ring.push(TraceRecord {
        seq,
        ts_ns: crate::drivers::timer::monotonic_ns_since_boot(),
        pid,
        cpu: cpu as u16,
        event_id: event.id,
        flags: render_flags(),
        comm,
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

/// The records of every CPU, in sequence order, without consuming
/// them (the `trace` file).
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
    out.sort_by_key(|r| r.seq);
    out
}

/// Take and consume the buffered records (the `trace_pipe` file: a
/// read spools the records out of the buffer).
pub fn drain() -> Vec<TraceRecord> {
    let mut out: Vec<TraceRecord> = Vec::new();
    for ring in RINGS.iter() {
        let count = ring.count.load(Ordering::Acquire).min(RING_CAPACITY);
        let head = ring.head.load(Ordering::Acquire);
        let start = (head + RING_CAPACITY - count) % RING_CAPACITY;
        for i in 0..count {
            let idx = (start + i) % RING_CAPACITY;
            out.push(ring.records[idx]);
        }
        // Consume: a racing writer may land one record into the slots
        // we just read; it is dropped with the reset — the same
        // spool-and-reset contract the reference host's pipe reader has.
        ring.head.store(0, Ordering::Release);
        ring.count.store(0, Ordering::Release);
    }
    out.sort_by_key(|r| r.seq);
    out
}

// ============================================================================
// The text renderer
// ============================================================================

/// The trace-file header, byte-for-byte the live reference host's (7.0
/// host; older kernels say "irqs-off" where this one says
/// "irqs-off/BH-disabled").
fn trace_header() -> String {
    let mut out = String::new();
    out.push_str("# tracer: nop\n");
    out.push_str("#\n");
    out.push_str(&format!(
        "# entries-in-buffer/entries-written: {}/{}   #P:{}\n",
        records_buffered(),
        records_written(),
        crate::arch::smp::num_started_cpus().min(crate::config::MAX_CPUS)
    ));
    out.push_str("#\n");
    out.push_str("#                                _-----=> irqs-off/BH-disabled\n");
    out.push_str("#                               / _----=> need-resched\n");
    out.push_str("#                              | / _---=> hardirq/softirq\n");
    out.push_str("#                              || / _--=> preempt-depth\n");
    out.push_str("#                              ||| / _-=> migrate-disable\n");
    out.push_str("#                              |||| /     delay\n");
    out.push_str("#           TASK-PID     CPU#  |||||  TIMESTAMP  FUNCTION\n");
    out.push_str("#              | |         |   |||||     |         |\n");
    out
}

/// One record line, the reference host's `%16s-%-7d [%03d] %c%c%c%c%c %5lu.%06lu:
/// %s: %s` shape.
fn render_record(record: &TraceRecord) -> String {
    let event = EVENTS.iter().find(|e| e.id == record.event_id).copied();
    let Some(event) = event else {
        return String::new();
    };
    // The TASK-PID column: 16-wide right-aligned comm (the idle task
    // shows <idle>), '-', 7-wide left-aligned pid.
    let comm = if record.pid == 0 {
        "<idle>".to_string()
    } else {
        let end = record
            .comm
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(COMM_LEN);
        String::from_utf8_lossy(&record.comm[..end]).into_owned()
    };
    format!(
        "{comm:>16}-{pid:<7} [{cpu:03}] {flags} {sec:5}.{usec:06}: {name}: {args}\n",
        comm = comm,
        pid = record.pid,
        cpu = record.cpu,
        flags = core::str::from_utf8(&record.flags).unwrap_or("....."),
        sec = record.ts_ns / 1_000_000_000,
        usec = (record.ts_ns % 1_000_000_000) / 1000,
        name = event.name,
        args = (event.fmt_args)(&record.args),
    )
}

/// Render the buffered records the way the reference host's `trace` file does
/// (header + every record). The `trace_pipe` reader uses
/// `render_pipe`, which has no header.
pub fn render_trace() -> String {
    let mut out = trace_header();
    for record in snapshot().iter() {
        out.push_str(&render_record(record));
    }
    out
}

/// Render and consume, the `trace_pipe` way (records only, no header).
pub fn render_pipe() -> String {
    let mut out = String::new();
    for record in drain().iter() {
        out.push_str(&render_record(record));
    }
    out
}

// ============================================================================
// The argument formatters
// ============================================================================

/// A comm carried in two argument slots back to bytes.
fn comm_from_words(lo: u64, hi: u64) -> [u8; COMM_LEN] {
    let mut out = [0u8; COMM_LEN];
    out[..8].copy_from_slice(&lo.to_le_bytes());
    out[8..].copy_from_slice(&hi.to_le_bytes());
    out
}

/// A comm as two little-endian words (the mirror of comm_from_words).
pub fn comm_to_words(comm: &[u8; COMM_LEN]) -> [u64; 2] {
    let mut lo = [0u8; 8];
    let mut hi = [0u8; 8];
    lo.copy_from_slice(&comm[..8]);
    hi.copy_from_slice(&comm[8..]);
    [u64::from_le_bytes(lo), u64::from_le_bytes(hi)]
}

/// The sched_switch prev_state letters — the reference host's __print_flags table
/// masked to the task-state bits ({1,S},{2,D},{4,T},{8,t},{0x10,X},
/// {0x20,Z},{0x40,P},{0x80,I}), "R" when no bit is set.
fn task_state_str(bits: u64) -> String {
    const LETTERS: [(u64, &str); 8] = [
        (0x1, "S"),
        (0x2, "D"),
        (0x4, "T"),
        (0x8, "t"),
        (0x10, "X"),
        (0x20, "Z"),
        (0x40, "P"),
        (0x80, "I"),
    ];
    let masked = bits & 0xff;
    let mut parts: Vec<&str> = Vec::new();
    for (bit, letter) in LETTERS.iter() {
        if masked & bit != 0 {
            parts.push(letter);
        }
    }
    if parts.is_empty() {
        "R".to_string()
    } else {
        parts.join("|")
    }
}

fn fmt_sys_enter(args: &[u64; TRACE_ARGS]) -> String {
    format!(
        "NR {} ({:x}, {:x}, {:x}, {:x}, {:x}, {:x})",
        args[0] as i64, args[1], args[2], args[3], args[4], args[5], args[6]
    )
}

fn fmt_sys_exit(args: &[u64; TRACE_ARGS]) -> String {
    format!("NR {} = {}", args[0] as i64, args[1] as i64)
}

fn fmt_sched_switch(args: &[u64; TRACE_ARGS]) -> String {
    let comm = |bytes: &[u8; COMM_LEN]| -> String {
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(COMM_LEN);
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    };
    let prev_comm = comm_from_words(args[0], args[1]);
    let next_comm = comm_from_words(args[5], args[6]);
    format!(
        "prev_comm={} prev_pid={} prev_prio={} prev_state={} ==> next_comm={} next_pid={} next_prio={}",
        comm(&prev_comm),
        args[2] as u32,
        args[3] as i32,
        task_state_str(args[4]),
        comm(&next_comm),
        args[7] as u32,
        args[8] as i32,
    )
}

/// sched_waking / sched_wakeup / sched_wakeup_new share one field set:
/// comm (two slots), pid, prio, target_cpu.
fn fmt_sched_wakeup(args: &[u64; TRACE_ARGS]) -> String {
    let comm = comm_from_words(args[0], args[1]);
    let end = comm.iter().position(|&b| b == 0).unwrap_or(COMM_LEN);
    format!(
        "comm={} pid={} prio={} target_cpu={:03}",
        String::from_utf8_lossy(&comm[..end]),
        args[2] as u32,
        args[3] as i32,
        args[4] as i32,
    )
}

/// sched_process_fork: parent comm (two slots), parent pid, child comm
/// (two slots), child pid.
fn fmt_sched_process_fork(args: &[u64; TRACE_ARGS]) -> String {
    let comm = |bytes: &[u8; COMM_LEN]| -> String {
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(COMM_LEN);
        String::from_utf8_lossy(&bytes[..end]).into_owned()
    };
    let parent = comm_from_words(args[0], args[1]);
    let child = comm_from_words(args[3], args[4]);
    format!(
        "comm={} pid={} child_comm={} child_pid={}",
        comm(&parent),
        args[2] as u32,
        comm(&child),
        args[5] as u32,
    )
}

/// sched_process_exit: comm (two slots), pid, prio, group_dead.
fn fmt_sched_process_exit(args: &[u64; TRACE_ARGS]) -> String {
    let comm = comm_from_words(args[0], args[1]);
    let end = comm.iter().position(|&b| b == 0).unwrap_or(COMM_LEN);
    format!(
        "comm={} pid={} prio={} group_dead={}",
        String::from_utf8_lossy(&comm[..end]),
        args[2] as u32,
        args[3] as i32,
        if args[4] != 0 { "true" } else { "false" },
    )
}

/// page_fault_user / page_fault_kernel: address, ip, error_code. The
/// `%ps` rendering is symbolic — the symbol table resolves the address
/// to name+off/size and falls back to the raw hex exactly as the
/// reference's %ps does when no symbol covers the address.
fn fmt_page_fault(args: &[u64; TRACE_ARGS]) -> String {
    let ps = |addr: u64| -> String {
        match crate::dfx::ksyms::lookup(addr) {
            Some((name, sym, size)) => {
                if size > 0 {
                    format!("{}+0x{:x}/0x{:x}", name, addr - sym, size)
                } else {
                    format!("{}+0x{:x}", name, addr - sym)
                }
            }
            None => format!("{:#x}", addr),
        }
    };
    format!(
        "address={} ip={} error_code=0x{:x}",
        ps(args[0]),
        ps(args[1]),
        args[2]
    )
}
