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
// One declaration site per event — the tracepoint! table
// ============================================================================
//
// Every event is declared ONCE, in the `tracepoints!` invocation below:
// its names, its id, the verbatim "print fmt" and field list, the typed
// call signature and the renderer. From that one declaration the macro
// derives the three artifacts a hand-written site used to carry:
//
// 1. the `static` table row (group/name/id/print_fmt/fields/enabled);
// 2. the record-call function `trace_<name>(field, field, ...) -> bool`
//    that packs the typed arguments into the argument slots and records
//    — the `armed()` check is inside, so a call site pays nothing but
//    the argument evaluation when the event is off;
// 3. the renderer `<name>_fmt` used by the text output and the format
//    file.
//
// This is the §4 derive pattern of docs/architecture/rust-design-headroom.md
// (one declaration derives every artifact, so the format and the data can
// no longer drift apart — the table row, the call signature and the file
// shapes are all one edit).

/// How a typed call argument packs into the argument slots.
pub trait PackArg {
    /// How many slots this argument occupies.
    const SLOTS: usize;
    /// Pack into `slots` starting at `*at`, advancing `at`.
    fn pack(&self, slots: &mut [u64; TRACE_ARGS], at: &mut usize);
}

macro_rules! pack_impl_scalar {
    ($($t:ty),*) => {$(
        impl PackArg for $t {
            const SLOTS: usize = 1;
            #[inline]
            fn pack(&self, slots: &mut [u64; TRACE_ARGS], at: &mut usize) {
                if *at < TRACE_ARGS {
                    slots[*at] = *self as u64;
                    *at += 1;
                }
            }
        }
    )*};
}

pack_impl_scalar!(u64, i64, u32, i32, u16, i16, u8, i8, usize, isize, bool);

/// A 16-byte comm occupies two slots (little-endian words — the mirror
/// of `comm_from_words`).
impl PackArg for &[u8; COMM_LEN] {
    const SLOTS: usize = 2;
    #[inline]
    fn pack(&self, slots: &mut [u64; TRACE_ARGS], at: &mut usize) {
        let words = comm_to_words(self);
        for w in words {
            if *at < TRACE_ARGS {
                slots[*at] = w;
                *at += 1;
            }
        }
    }
}

/// A short ASCII tag (the rwbs column: "R", "W", ...) packs into one
/// slot as up to 8 little-endian bytes.
impl PackArg for &str {
    const SLOTS: usize = 1;
    #[inline]
    fn pack(&self, slots: &mut [u64; TRACE_ARGS], at: &mut usize) {
        let mut chunk = [0u8; 8];
        let n = self.len().min(8);
        chunk[..n].copy_from_slice(&self.as_bytes()[..n]);
        if *at < TRACE_ARGS {
            slots[*at] = u64::from_le_bytes(chunk);
            *at += 1;
        }
    }
}

/// Declare the trace event table. One item per event:
///
/// ```text
/// EVENT_STATIC / name_fmt / trace_name : "group", "name", id,
///     print:  "<verbatim print fmt>",
///     fields: "<verbatim field lines>",
///     call    (field: ty, ...),
///     render |args: &[u64; TRACE_ARGS]| { <expr using `args`> } ;
/// ```
///
/// `EVENT_STATIC / name_fmt / trace_name` are the three derived names
/// (the table row, the renderer, the call function) — written out
/// because macro_rules cannot concatenate identifiers; every other
/// artifact is derived. The event table `EVENTS` is emitted from the
/// same list, in declaration order.
macro_rules! tracepoints {
    // Munch one event; emit its three artifacts; accumulate the static.
    // (The @-marked arms must precede the catch-all entry arm below —
    // macro_rules tries arms in order and the entry arm matches any
    // token sequence, including an @tbl form.)
    (
        @tbl [$($acc:ident)*]
        $(#[$doc:meta])*
        $ev:ident / $fmt:ident / $call:ident : $group:literal, $name:literal, $id:literal,
        print: $print:literal,
        fields: $fields:literal,
        call ($($pn:ident : $pt:ty),* $(,)?),
        render $render:expr
        ; $($rest:tt)*
    ) => {
        $(#[$doc])*
        pub static $ev: TraceEvent = TraceEvent {
            group: $group,
            name: $name,
            id: $id,
            print_fmt: $print,
            fields: $fields,
            enabled: AtomicBool::new(false),
            fmt_args: $fmt,
        };

        #[allow(unused_variables)]
        fn $fmt(args: &[u64; TRACE_ARGS]) -> String {
            // `render |args| ...`: the renderer is a closure written at
            // the declaration site, so `args` binds there (macro_rules
            // hygiene would otherwise hide the parameter from it).
            ($render)(args)
        }

        /// Record this event; the typed arguments are packed into the
        /// argument slots. The `armed()` check is FIRST, so a call site
        /// pays nothing but its argument evaluation when the event is
        /// off — the same contract the hand-written sites have.
        #[inline]
        pub fn $call($($pn: $pt),*) -> bool {
            const _: () = assert!(0 $(+ <$pt as PackArg>::SLOTS)* <= TRACE_ARGS);
            if !$ev.armed() {
                return false;
            }
            let mut slots = [0u64; TRACE_ARGS];
            let mut at = 0usize;
            $($pn.pack(&mut slots, &mut at);)*
            record(&$ev, slots)
        }

        tracepoints!(@tbl [$($acc)* $ev] $($rest)*);
    };

    // Base case: every event declared — emit the table.
    (@tbl [$($acc:ident)+]) => {
        /// Every event, in declaration (id) order.
        pub static EVENTS: &[&TraceEvent] = &[$(&$acc),*];
    };

    // Entry (last on purpose — it matches any token sequence): start
    // the table walk with an empty accumulator.
    ($($item:tt)*) => {
        tracepoints!(@tbl [] $($item)*);
    };
}

tracepoints! {
    /// raw_syscalls:sys_enter — every syscall entry, with the raw
    /// (native) number and the six register arguments. the reference
    /// host's events/raw_syscalls/sys_enter, format captured from the
    /// live file.
    EVENT_SYS_ENTER / sys_enter_fmt / trace_sys_enter : "raw_syscalls", "sys_enter", 1,
    print: "\"NR %ld (%lx, %lx, %lx, %lx, %lx, %lx)\", REC->id, REC->args[0], REC->args[1], REC->args[2], REC->args[3], REC->args[4], REC->args[5]",
    fields: "\tfield:long id;\toffset:8;\tsize:8;\tsigned:1;\n\
             \tfield:unsigned long args[6];\toffset:16;\tsize:48;\tsigned:0;\n",
    call (id: i64, arg0: u64, arg1: u64, arg2: u64, arg3: u64, arg4: u64, arg5: u64),
    render |args: &[u64; TRACE_ARGS]| {
        format!(
            "NR {} ({:x}, {:x}, {:x}, {:x}, {:x}, {:x})",
            args[0] as i64, args[1], args[2], args[3], args[4], args[5], args[6]
        )
    };

    /// raw_syscalls:sys_exit — every syscall return, with the raw number
    /// and the return value. the reference host's
    /// events/raw_syscalls/sys_exit.
    EVENT_SYS_EXIT / sys_exit_fmt / trace_sys_exit : "raw_syscalls", "sys_exit", 2,
    print: "\"NR %ld = %ld\", REC->id, REC->ret",
    fields: "\tfield:long id;\toffset:8;\tsize:8;\tsigned:1;\n\
             \tfield:long ret;\toffset:16;\tsize:8;\tsigned:1;\n",
    call (id: i64, ret: i64),
    render |args: &[u64; TRACE_ARGS]| { format!("NR {} = {}", args[0] as i64, args[1] as i64) };

    /// sched:sched_switch — a context switch. the reference host's
    /// events/sched/sched_switch; the print fmt is the live file's blob
    /// verbatim (the __print_flags expression for prev_state).
    EVENT_SCHED_SWITCH / sched_switch_fmt / trace_sched_switch : "sched", "sched_switch", 3,
    print: "\"prev_comm=%s prev_pid=%d prev_prio=%d prev_state=%s%s ==> next_comm=%s next_pid=%d next_prio=%d\", REC->prev_comm, REC->prev_pid, REC->prev_prio, (REC->prev_state & ((((0x00000000 | 0x00000001 | 0x00000002 | 0x00000004 | 0x00000008 | 0x00000010 | 0x00000020 | 0x00000040) + 1) << 1) - 1)) ? __print_flags(REC->prev_state & ((((0x00000000 | 0x00000001 | 0x00000002 | 0x00000004 | 0x00000008 | 0x00000010 | 0x00000020 | 0x00000040) + 1) << 1) - 1), \"|\", { 0x00000001, \"S\" }, { 0x00000002, \"D\" }, { 0x00000004, \"T\" }, { 0x00000008, \"t\" }, { 0x00000010, \"X\" }, { 0x00000020, \"Z\" }, { 0x00000040, \"P\" }, { 0x00000080, \"I\" }) : \"R\", REC->prev_state & (((0x00000000 | 0x00000001 | 0x00000002 | 0x00000004 | 0x00000008 | 0x00000010 | 0x00000020 | 0x00000040) + 1) << 1) ? \"+\" : \"\", REC->next_comm, REC->next_pid, REC->next_prio",
    fields: "\tfield:char prev_comm[16];\toffset:8;\tsize:16;\tsigned:0;\n\
             \tfield:pid_t prev_pid;\toffset:24;\tsize:4;\tsigned:1;\n\
             \tfield:int prev_prio;\toffset:28;\tsize:4;\tsigned:1;\n\
             \tfield:long prev_state;\toffset:32;\tsize:8;\tsigned:1;\n\
             \tfield:char next_comm[16];\toffset:40;\tsize:16;\tsigned:0;\n\
             \tfield:pid_t next_pid;\toffset:56;\tsize:4;\tsigned:1;\n\
             \tfield:int next_prio;\toffset:60;\tsize:4;\tsigned:1;\n",
    call (prev_comm: &[u8; COMM_LEN], prev_pid: u32, prev_prio: i32, prev_state: u64,
          next_comm: &[u8; COMM_LEN], next_pid: u32, next_prio: i32),
    render |args: &[u64; TRACE_ARGS]| {
        let comm = |bytes: &[u8; COMM_LEN]| -> String {
            let end = bytes.iter().position(|&b| b == 0).unwrap_or(COMM_LEN);
            String::from_utf8_lossy(&bytes[..end]).into_owned()
        };
        let pc = comm_from_words(args[0], args[1]);
        let nc = comm_from_words(args[5], args[6]);
        format!(
            "prev_comm={} prev_pid={} prev_prio={} prev_state={} ==> next_comm={} next_pid={} next_prio={}",
            comm(&pc),
            args[2] as u32,
            args[3] as i32,
            task_state_str(args[4]),
            comm(&nc),
            args[7] as u32,
            args[8] as i32,
        )
    };

    /// sched:sched_waking — a task is about to be woken (emitted before the
    /// enqueue attempt). The reference host's events/sched/sched_waking;
    /// the fields and print fmt are the live file's, identical to
    /// sched_wakeup's.
    EVENT_SCHED_WAKING / sched_waking_fmt / trace_sched_waking : "sched", "sched_waking", 4,
    print: "\"comm=%s pid=%d prio=%d target_cpu=%03d\", REC->comm, REC->pid, REC->prio, REC->target_cpu",
    fields: "\tfield:char comm[16];\toffset:8;\tsize:16;\tsigned:0;\n\
             \tfield:pid_t pid;\toffset:24;\tsize:4;\tsigned:1;\n\
             \tfield:int prio;\toffset:28;\tsize:4;\tsigned:1;\n\
             \tfield:int target_cpu;\toffset:32;\tsize:4;\tsigned:1;\n",
    call (comm: &[u8; COMM_LEN], pid: u32, prio: i32, target_cpu: i32),
    render |args: &[u64; TRACE_ARGS]| { fmt_sched_wakeup(args) };

    /// sched:sched_wakeup — a task left the wait queue and was enqueued
    /// (recorded after the enqueue succeeded). The reference host's
    /// events/sched/sched_wakeup.
    EVENT_SCHED_WAKEUP / sched_wakeup_fmt / trace_sched_wakeup : "sched", "sched_wakeup", 5,
    print: "\"comm=%s pid=%d prio=%d target_cpu=%03d\", REC->comm, REC->pid, REC->prio, REC->target_cpu",
    fields: "\tfield:char comm[16];\toffset:8;\tsize:16;\tsigned:0;\n\
             \tfield:pid_t pid;\toffset:24;\tsize:4;\tsigned:1;\n\
             \tfield:int prio;\toffset:28;\tsize:4;\tsigned:1;\n\
             \tfield:int target_cpu;\toffset:32;\tsize:4;\tsigned:1;\n",
    call (comm: &[u8; COMM_LEN], pid: u32, prio: i32, target_cpu: i32),
    render |args: &[u64; TRACE_ARGS]| { fmt_sched_wakeup(args) };

    /// sched:sched_wakeup_new — the FIRST enqueue of a freshly built task
    /// (the fork path's enqueue). The reference host's
    /// events/sched/sched_wakeup_new; fields identical to sched_wakeup's.
    EVENT_SCHED_WAKEUP_NEW / sched_wakeup_new_fmt / trace_sched_wakeup_new : "sched", "sched_wakeup_new", 6,
    print: "\"comm=%s pid=%d prio=%d target_cpu=%03d\", REC->comm, REC->pid, REC->prio, REC->target_cpu",
    fields: "\tfield:char comm[16];\toffset:8;\tsize:16;\tsigned:0;\n\
             \tfield:pid_t pid;\toffset:24;\tsize:4;\tsigned:1;\n\
             \tfield:int prio;\toffset:28;\tsize:4;\tsigned:1;\n\
             \tfield:int target_cpu;\toffset:32;\tsize:4;\tsigned:1;\n",
    call (comm: &[u8; COMM_LEN], pid: u32, prio: i32, target_cpu: i32),
    render |args: &[u64; TRACE_ARGS]| { fmt_sched_wakeup(args) };

    /// sched:sched_process_fork — a child task was built. The reference
    /// host's events/sched/sched_process_fork; the __data_loc char[]
    /// fields are the live file's (a dynamic string carries its blob
    /// offset in the field word; Rux records the comms inline instead and
    /// renders them the same way).
    EVENT_SCHED_PROCESS_FORK / sched_process_fork_fmt / trace_sched_process_fork : "sched", "sched_process_fork", 7,
    print: "\"comm=%s pid=%d child_comm=%s child_pid=%d\", __get_str(parent_comm), REC->parent_pid, __get_str(child_comm), REC->child_pid",
    fields: "\tfield:__data_loc char[] parent_comm;\toffset:8;\tsize:4;\tsigned:0;\n\
             \tfield:pid_t parent_pid;\toffset:12;\tsize:4;\tsigned:1;\n\
             \tfield:__data_loc char[] child_comm;\toffset:16;\tsize:4;\tsigned:0;\n\
             \tfield:pid_t child_pid;\toffset:20;\tsize:4;\tsigned:1;\n",
    call (parent_comm: &[u8; COMM_LEN], parent_pid: u32, child_comm: &[u8; COMM_LEN], child_pid: u32),
    render |args: &[u64; TRACE_ARGS]| { fmt_sched_process_fork(args) };

    /// sched:sched_process_exit — a task entered do_exit. The reference
    /// host's events/sched/sched_process_exit.
    EVENT_SCHED_PROCESS_EXIT / sched_process_exit_fmt / trace_sched_process_exit : "sched", "sched_process_exit", 8,
    print: "\"comm=%s pid=%d prio=%d group_dead=%s\", REC->comm, REC->pid, REC->prio, REC->group_dead ? \"true\" : \"false\"",
    fields: "\tfield:char comm[16];\toffset:8;\tsize:16;\tsigned:0;\n\
             \tfield:pid_t pid;\toffset:24;\tsize:4;\tsigned:1;\n\
             \tfield:int prio;\toffset:28;\tsize:4;\tsigned:1;\n\
             \tfield:bool group_dead;\toffset:32;\tsize:1;\tsigned:0;\n",
    call (comm: &[u8; COMM_LEN], pid: u32, prio: i32, group_dead: bool),
    render |args: &[u64; TRACE_ARGS]| { fmt_sched_process_exit(args) };

    /// exceptions:page_fault_user — a page fault taken from user mode. The
    /// reference host does not carry the exceptions group (it is the
    /// architecture fault-trace family); the field set and print fmt are
    /// the generic definition's (address/ip/error_code, %ps symbolic
    /// rendering). On riscv64 the error_code carries the access-type bits
    /// (read=0x1 write=0x2 exec=0x4) rather than the x86 error-word, and
    /// ip is the faulting epc.
    EVENT_PAGE_FAULT_USER / page_fault_user_fmt / trace_page_fault_user : "exceptions", "page_fault_user", 9,
    print: "\"address=%ps ip=%ps error_code=0x%lx\", (void *)REC->address, (void *)REC->ip, REC->error_code",
    fields: "\tfield:unsigned long address;\toffset:8;\tsize:8;\tsigned:0;\n\
             \tfield:unsigned long ip;\toffset:16;\tsize:8;\tsigned:0;\n\
             \tfield:unsigned long error_code;\toffset:24;\tsize:8;\tsigned:0;\n",
    call (address: u64, ip: u64, error_code: u64),
    render |args: &[u64; TRACE_ARGS]| { fmt_page_fault(args) };

    /// exceptions:page_fault_kernel — a page fault taken from kernel mode
    /// (a vmalloc-style fault or a kernel bug). Same shape as
    /// page_fault_user's.
    EVENT_PAGE_FAULT_KERNEL / page_fault_kernel_fmt / trace_page_fault_kernel : "exceptions", "page_fault_kernel", 10,
    print: "\"address=%ps ip=%ps error_code=0x%lx\", (void *)REC->address, (void *)REC->ip, REC->error_code",
    fields: "\tfield:unsigned long address;\toffset:8;\tsize:8;\tsigned:0;\n\
             \tfield:unsigned long ip;\toffset:16;\tsize:8;\tsigned:0;\n\
             \tfield:unsigned long error_code;\toffset:24;\tsize:8;\tsigned:0;\n",
    call (address: u64, ip: u64, error_code: u64),
    render |args: &[u64; TRACE_ARGS]| { fmt_page_fault(args) };

    /// block:block_rq_issue — a block request was handed to the device.
    /// The reference host's events/block/block_rq_issue; the field set
    /// and print fmt are the live file's verbatim. Rux merges no
    /// requests: every buffer-head transfer IS one request. `cmd` is the
    /// SCSI command bytes the reference attaches — virtio-blk carries
    /// none, so the dynamic string is empty and renders as `()`. Rux has
    /// no ioprio; the field reports 0, which the reference's
    /// __print_symbolic renders as the `none` class. `rwbs` carries the
    /// transfer direction only (R read / W write / F flush) — the
    /// reference's sync/meta bits have no counterpart here.
    EVENT_BLOCK_RQ_ISSUE / block_rq_issue_fmt / trace_block_rq_issue : "block", "block_rq_issue", 12,
    print: "\"%d,%d %s %u (%s) %llu + %u %s,%u,%u [%s]\", ((unsigned int) ((REC->dev) >> 20)), ((unsigned int) ((REC->dev) & ((1U << 20) - 1))), REC->rwbs, REC->bytes, __get_str(cmd), (unsigned long long)REC->sector, REC->nr_sector, __print_symbolic((((REC->ioprio) >> 13) & (8 - 1)), { IOPRIO_CLASS_NONE, \"none\" }, { IOPRIO_CLASS_RT, \"rt\" }, { IOPRIO_CLASS_BE, \"be\" }, { IOPRIO_CLASS_IDLE, \"idle\" }, { IOPRIO_CLASS_INVALID, \"invalid\"}), (((REC->ioprio) >> 3) & ((1 << 10) - 1)), ((REC->ioprio) & ((1 << 3) - 1)), REC->comm",
    fields: "\tfield:dev_t dev;\toffset:8;\tsize:4;\tsigned:0;\n\
             \tfield:sector_t sector;\toffset:16;\tsize:8;\tsigned:0;\n\
             \tfield:unsigned int nr_sector;\toffset:24;\tsize:4;\tsigned:0;\n\
             \tfield:unsigned int bytes;\toffset:28;\tsize:4;\tsigned:0;\n\
             \tfield:unsigned short ioprio;\toffset:32;\tsize:2;\tsigned:0;\n\
             \tfield:char rwbs[10];\toffset:34;\tsize:10;\tsigned:0;\n\
             \tfield:char comm[16];\toffset:44;\tsize:16;\tsigned:0;\n\
             \tfield:__data_loc char[] cmd;\toffset:60;\tsize:4;\tsigned:0;\n",
    call (dev: u32, sector: u64, nr_sector: u32, bytes: u32, ioprio: u16,
          rwbs: &str, comm: &[u8; COMM_LEN]),
    render |args: &[u64; TRACE_ARGS]| {
        let comm = comm_from_words(args[6], args[7]);
        let cend = comm.iter().position(|&b| b == 0).unwrap_or(COMM_LEN);
        format!(
            "{},{} {} {} () {} + {} {},{},{} [{}]",
            (args[0] >> 20) as u32,
            (args[0] & 0xfffff) as u32,
            ascii_from_word(args[5]),
            args[3] as u32,
            args[1],
            args[2] as u32,
            ioprio_class(args[4] as u16),
            (args[4] >> 3) & 0x3ff,
            args[4] & 7,
            String::from_utf8_lossy(&comm[..cend]),
        )
    };

    /// block:block_rq_complete — a block request finished at the device.
    /// The reference host's events/block/block_rq_complete; field set and
    /// print fmt verbatim from the live file. The complete fires from
    /// the completion path (softirq for the async reads), so the
    /// TASK-PID column shows the delivering context, not the issuer —
    /// the reference behaves the same way.
    EVENT_BLOCK_RQ_COMPLETE / block_rq_complete_fmt / trace_block_rq_complete : "block", "block_rq_complete", 13,
    print: "\"%d,%d %s (%s) %llu + %u %s,%u,%u [%d]\", ((unsigned int) ((REC->dev) >> 20)), ((unsigned int) ((REC->dev) & ((1U << 20) - 1))), REC->rwbs, __get_str(cmd), (unsigned long long)REC->sector, REC->nr_sector, __print_symbolic((((REC->ioprio) >> 13) & (8 - 1)), { IOPRIO_CLASS_NONE, \"none\" }, { IOPRIO_CLASS_RT, \"rt\" }, { IOPRIO_CLASS_BE, \"be\" }, { IOPRIO_CLASS_IDLE, \"idle\" }, { IOPRIO_CLASS_INVALID, \"invalid\"}), (((REC->ioprio) >> 3) & ((1 << 10) - 1)), ((REC->ioprio) & ((1 << 3) - 1)), REC->error",
    fields: "\tfield:dev_t dev;\toffset:8;\tsize:4;\tsigned:0;\n\
             \tfield:sector_t sector;\toffset:16;\tsize:8;\tsigned:0;\n\
             \tfield:unsigned int nr_sector;\toffset:24;\tsize:4;\tsigned:0;\n\
             \tfield:int error;\toffset:28;\tsize:4;\tsigned:1;\n\
             \tfield:unsigned short ioprio;\toffset:32;\tsize:2;\tsigned:0;\n\
             \tfield:char rwbs[10];\toffset:34;\tsize:10;\tsigned:0;\n\
             \tfield:__data_loc char[] cmd;\toffset:44;\tsize:4;\tsigned:0;\n",
    call (dev: u32, sector: u64, nr_sector: u32, error: i32, ioprio: u16, rwbs: &str),
    render |args: &[u64; TRACE_ARGS]| {
        format!(
            "{},{} {} () {} + {} {},{},{} {}",
            (args[0] >> 20) as u32,
            (args[0] & 0xfffff) as u32,
            ascii_from_word(args[5]),
            args[1],
            args[2] as u32,
            ioprio_class(args[4] as u16),
            (args[4] >> 3) & 0x3ff,
            args[4] & 7,
            args[3] as i32,
        )
    };

    /// binder:binder_transaction — a transaction was queued to its
    /// target. The reference kernel's drivers/android binder trace
    /// definitions (this host's tracefs has no binder group — the
    /// binder family is not compiled in here — so the field set and
    /// print fmt are the reference definitions' verbatim). `to_thread`
    /// is 0 at this point (the reference records the same: the
    /// receiving thread is only chosen at delivery).
    EVENT_BINDER_TRANSACTION / binder_transaction_fmt / trace_binder_transaction : "binder", "binder_transaction", 14,
    print: "\"transaction=%d dest_node=%d dest_proc=%d dest_thread=%d reply=%d flags=0x%x code=0x%x\", __entry->debug_id, __entry->target_node, __entry->to_proc, __entry->to_thread, __entry->reply, __entry->flags, __entry->code",
    fields: "\tfield:int debug_id;\toffset:8;\tsize:4;\tsigned:1;\n\
             \tfield:int target_node;\toffset:12;\tsize:4;\tsigned:1;\n\
             \tfield:int to_proc;\toffset:16;\tsize:4;\tsigned:1;\n\
             \tfield:int to_thread;\toffset:20;\tsize:4;\tsigned:1;\n\
             \tfield:int reply;\toffset:24;\tsize:4;\tsigned:1;\n\
             \tfield:unsigned int code;\toffset:28;\tsize:4;\tsigned:0;\n\
             \tfield:unsigned int flags;\toffset:32;\tsize:4;\tsigned:0;\n",
    call (debug_id: u64, target_node: u64, to_proc: u32, to_thread: u32,
          reply: bool, code: u32, flags: u32),
    render |args: &[u64; TRACE_ARGS]| {
        format!(
            "transaction={} dest_node={} dest_proc={} dest_thread={} reply={} flags={:#x} code={:#x}",
            args[0], args[1], args[2], args[3], args[4], args[6], args[5]
        )
    };

    /// binder:binder_transaction_received — a transaction was handed to
    /// its receiving thread (the BR_TRANSACTION/BR_REPLY read-out). Same
    /// provenance as binder_transaction.
    EVENT_BINDER_TRANSACTION_RECEIVED / binder_transaction_received_fmt / trace_binder_transaction_received : "binder", "binder_transaction_received", 15,
    print: "\"transaction=%d\", __entry->debug_id",
    fields: "\tfield:int debug_id;\toffset:8;\tsize:4;\tsigned:1;\n",
    call (debug_id: u64),
    render |args: &[u64; TRACE_ARGS]| { format!("transaction={}", args[0]) };

    /// binder:binder_transaction_alloc_buf — the target's mmap buffer was
    /// carved for a transaction. The reference's binder_buffer class
    /// (shared by the alloc/release family): debug id, data size,
    /// offsets size, extra (scatter-gather) size.
    EVENT_BINDER_TRANSACTION_ALLOC_BUF / binder_transaction_alloc_buf_fmt / trace_binder_transaction_alloc_buf : "binder", "binder_transaction_alloc_buf", 16,
    print: "\"transaction=%d data_size=%zd offsets_size=%zd extra_buffers_size=%zd\", __entry->debug_id, __entry->data_size, __entry->offsets_size, __entry->extra_buffers_size",
    fields: "\tfield:int debug_id;\toffset:8;\tsize:4;\tsigned:1;\n\
             \tfield:size_t data_size;\toffset:16;\tsize:8;\tsigned:0;\n\
             \tfield:size_t offsets_size;\toffset:24;\tsize:8;\tsigned:0;\n\
             \tfield:size_t extra_buffers_size;\toffset:32;\tsize:8;\tsigned:0;\n",
    call (debug_id: u64, data_size: u64, offsets_size: u64, extra_buffers_size: u64),
    render |args: &[u64; TRACE_ARGS]| {
        format!(
            "transaction={} data_size={} offsets_size={} extra_buffers_size={}",
            args[0], args[1], args[2], args[3]
        )
    };

    /// binder:binder_transaction_buffer_release — a delivered buffer was
    /// released (the receiver's BC_FREE_BUFFER, or the teardown paths
    /// that free through the same chokepoint). Same field set as
    /// alloc_buf's.
    EVENT_BINDER_TRANSACTION_BUFFER_RELEASE / binder_transaction_buffer_release_fmt / trace_binder_transaction_buffer_release : "binder", "binder_transaction_buffer_release", 17,
    print: "\"transaction=%d data_size=%zd offsets_size=%zd extra_buffers_size=%zd\", __entry->debug_id, __entry->data_size, __entry->offsets_size, __entry->extra_buffers_size",
    fields: "\tfield:int debug_id;\toffset:8;\tsize:4;\tsigned:1;\n\
             \tfield:size_t data_size;\toffset:16;\tsize:8;\tsigned:0;\n\
             \tfield:size_t offsets_size;\toffset:24;\tsize:8;\tsigned:0;\n\
             \tfield:size_t extra_buffers_size;\toffset:32;\tsize:8;\tsigned:0;\n",
    call (debug_id: u64, data_size: u64, offsets_size: u64, extra_buffers_size: u64),
    render |args: &[u64; TRACE_ARGS]| {
        format!(
            "transaction={} data_size={} offsets_size={} extra_buffers_size={}",
            args[0], args[1], args[2], args[3]
        )
    };

    /// sched:sched_process_free — the LAST reference to a task dropped
    /// and its slot is about to be freed (the reference's
    /// delayed_put_task_struct → free_task, recorded at the final put).
    /// Closes the fork→exit→free lifecycle: a task that exits but never
    /// frees is the leak-hunt signature. Same template as
    /// sched_process_exit (the reference's sched_process_template).
    EVENT_SCHED_PROCESS_FREE / sched_process_free_fmt / trace_sched_process_free : "sched", "sched_process_free", 18,
    print: "\"comm=%s pid=%d prio=%d\", __get_str(comm), REC->pid, REC->prio",
    fields: "\tfield:__data_loc char[] comm;\toffset:8;\tsize:4;\tsigned:0;\n\
             \tfield:pid_t pid;\toffset:12;\tsize:4;\tsigned:1;\n\
             \tfield:int prio;\toffset:16;\tsize:4;\tsigned:1;\n",
    call (comm: &[u8; COMM_LEN], pid: u32, prio: i32),
    render |args: &[u64; TRACE_ARGS]| {
        let comm = comm_from_words(args[0], args[1]);
        let end = comm.iter().position(|&b| b == 0).unwrap_or(COMM_LEN);
        format!(
            "comm={} pid={} prio={}",
            String::from_utf8_lossy(&comm[..end]),
            args[2] as u32,
            args[3] as i32,
        )
    };

    /// sched:sched_process_wait — a task entered do_wait. The reference
    /// records the WAITER's comm/prio with the REQUESTED pid (the
    /// wait4/waitid pid argument, -1 for any child), at do_wait entry.
    EVENT_SCHED_PROCESS_WAIT / sched_process_wait_fmt / trace_sched_process_wait : "sched", "sched_process_wait", 19,
    print: "\"comm=%s pid=%d prio=%d\", __get_str(comm), REC->pid, REC->prio",
    fields: "\tfield:__data_loc char[] comm;\toffset:8;\tsize:4;\tsigned:0;\n\
             \tfield:pid_t pid;\toffset:12;\tsize:4;\tsigned:1;\n\
             \tfield:int prio;\toffset:16;\tsize:4;\tsigned:1;\n",
    call (comm: &[u8; COMM_LEN], pid: i32, prio: i32),
    render |args: &[u64; TRACE_ARGS]| {
        let comm = comm_from_words(args[0], args[1]);
        let end = comm.iter().position(|&b| b == 0).unwrap_or(COMM_LEN);
        format!(
            "comm={} pid={} prio={}",
            String::from_utf8_lossy(&comm[..end]),
            args[2] as i32,
            args[3] as i32,
        )
    };

    /// binder:binder_wait_for_work — a binder reader thread is about to
    /// park waiting for work. The reference's drivers/android/binder.c
    /// binder_thread_read records it just before the blocking wait with
    /// the three availability words (this host has no binder tracefs
    /// group; the definition is binder_trace.h's, 5.15). The wedge
    /// forensics pair: a wait_for_work with no matching work arrival
    /// afterwards names the parked thread that never woke.
    EVENT_BINDER_WAIT_FOR_WORK / binder_wait_for_work_fmt / trace_binder_wait_for_work : "binder", "binder_wait_for_work", 20,
    print: "\"proc_work=%d transaction_stack=%d thread_todo=%d\", REC->proc_work, REC->transaction_stack, REC->thread_todo",
    fields: "\tfield:bool proc_work;\toffset:8;\tsize:1;\tsigned:0;\n\
             \tfield:bool transaction_stack;\toffset:9;\tsize:1;\tsigned:0;\n\
             \tfield:bool thread_todo;\toffset:10;\tsize:1;\tsigned:0;\n",
    call (proc_work: bool, transaction_stack: bool, thread_todo: bool),
    render |args: &[u64; TRACE_ARGS]| {
        format!(
            "proc_work={} transaction_stack={} thread_todo={}",
            args[0], args[1], args[2]
        )
    };

    /// signal:signal_generate — a signal was sent to a task, with the
    /// disposition outcome. The reference fires it once per __send_signal
    /// with result: 0 delivered, 1 ignored, 2 already pending (standard
    /// signals coalesce), 3 overflow-info, 4 overflow-fail (EAGAIN,
    /// RLIMIT_SIGPENDING), 5 lose-info. group: 1 = thread-group-directed
    /// (kill/SIGCHLD family), 0 = directed at the one thread
    /// (tkill/force_sig/SIGPIPE/timer). The kill-storm forensics event:
    /// every OOM SIGKILL and teardown kill lands here with both ends
    /// named.
    EVENT_SIGNAL_GENERATE / signal_generate_fmt / trace_signal_generate : "signal", "signal_generate", 21,
    print: "\"sig=%d errno=%d code=%d comm=%s pid=%d grp=%d res=%d\", REC->sig, REC->errno, REC->code, REC->comm, REC->pid, REC->group, REC->result",
    fields: "\tfield:int sig;\toffset:8;\tsize:4;\tsigned:1;\n\
             \tfield:int errno;\toffset:12;\tsize:4;\tsigned:1;\n\
             \tfield:int code;\toffset:16;\tsize:4;\tsigned:1;\n\
             \tfield:char comm[16];\toffset:20;\tsize:16;\tsigned:0;\n\
             \tfield:pid_t pid;\toffset:36;\tsize:4;\tsigned:1;\n\
             \tfield:int group;\toffset:40;\tsize:4;\tsigned:1;\n\
             \tfield:int result;\toffset:44;\tsize:4;\tsigned:1;\n",
    call (sig: i32, errno: i32, code: i32, comm: &[u8; COMM_LEN], pid: u32, group: i32, result: i32),
    render |args: &[u64; TRACE_ARGS]| {
        let comm = comm_from_words(args[3], args[4]);
        let end = comm.iter().position(|&b| b == 0).unwrap_or(COMM_LEN);
        format!(
            "sig={} errno={} code={} comm={} pid={} grp={} res={}",
            args[0] as i32,
            args[1] as i32,
            args[2] as i32,
            String::from_utf8_lossy(&comm[..end]),
            args[5] as u32,
            args[6] as i32,
            args[7] as i32,
        )
    };

    /// oom:mark_victim — the OOM killer committed to a victim. The
    /// reference's mm/oom_kill.c records it when the kill decision is
    /// final. Field honesty: Rux's mm keeps ONE resident-page counter
    /// (fault-in inc / unmap dec, anon + file together); it lands in
    /// anon_rss (the dominant class for victims). file_rss, shmem_rss
    /// and pgtables have no per-mm counterpart and report 0 — the split
    /// is not tracked per-mm yet. The print keeps the reference's
    /// literal `file-rss:` colon quirk verbatim.
    EVENT_MARK_VICTIM / mark_victim_fmt / trace_mark_victim : "oom", "mark_victim", 22,
    print: "\"pid=%d comm=%s total-vm=%lukB anon-rss=%lukB file-rss:%lukB shmem-rss=%lukB uid=%u pgtables=%lukB oom_score_adj=%hd\", REC->pid, __get_str(comm), REC->total_vm, REC->anon_rss, REC->file_rss, REC->shmem_rss, REC->uid, REC->pgtables, REC->oom_score_adj",
    fields: "\tfield:int pid;\toffset:8;\tsize:4;\tsigned:1;\n\
             \tfield:__data_loc char[] comm;\toffset:12;\tsize:4;\tsigned:0;\n\
             \tfield:unsigned long total_vm;\toffset:16;\tsize:8;\tsigned:0;\n\
             \tfield:unsigned long anon_rss;\toffset:24;\tsize:8;\tsigned:0;\n\
             \tfield:unsigned long file_rss;\toffset:32;\tsize:8;\tsigned:0;\n\
             \tfield:unsigned long shmem_rss;\toffset:40;\tsize:8;\tsigned:0;\n\
             \tfield:uid_t uid;\toffset:48;\tsize:4;\tsigned:0;\n\
             \tfield:unsigned long pgtables;\toffset:56;\tsize:8;\tsigned:0;\n\
             \tfield:short oom_score_adj;\toffset:64;\tsize:2;\tsigned:1;\n",
    call (pid: u32, comm: &[u8; COMM_LEN], total_vm: u64, anon_rss: u64, file_rss: u64,
          shmem_rss: u64, uid: u32, pgtables: u64, oom_score_adj: i16),
    render |args: &[u64; TRACE_ARGS]| {
        let comm = comm_from_words(args[1], args[2]);
        let end = comm.iter().position(|&b| b == 0).unwrap_or(COMM_LEN);
        format!(
            "pid={} comm={} total-vm={}kB anon-rss={}kB file-rss:{}kB shmem-rss={}kB uid={} pgtables={}kB oom_score_adj={}",
            args[0] as u32,
            String::from_utf8_lossy(&comm[..end]),
            args[3],
            args[4],
            args[5],
            args[6],
            args[7] as u32,
            args[8],
            args[9] as i16,
        )
    };

    /// kmem:kmalloc — one kernel heap allocation, with the ORIGIN
    /// (call_site, `%pS`-symbolized) the heap-leak hunts want: every
    /// Box/Vec/String build flows through the one GlobalAlloc
    /// chokepoint, so this single event names the allocating function.
    /// The reference host's events/kmem/kmalloc, field list and print
    /// fmt verbatim (the `accounted` column is DERIVED from
    /// __GFP_ACCOUNT there — no field row — and the same derivation
    /// runs here). Field honesty: Rux's allocator takes no gfp mask;
    /// the sites pass GFP_KERNEL (the analog of a plain kernel
    /// allocation), so gfp_flags renders "GFP_KERNEL" and accounted
    /// renders false — the reference's rendering of that mask. node is
    /// NUMA_NO_NODE (-1): Rux has no NUMA policy.
    EVENT_KMALLOC / kmalloc_fmt / trace_kmalloc : "kmem", "kmalloc", 23,
    print: "\"call_site=%pS ptr=%p bytes_req=%zu bytes_alloc=%zu gfp_flags=%s node=%d accounted=%s\", (void *)REC->call_site, REC->ptr, REC->bytes_req, REC->bytes_alloc, (REC->gfp_flags) ? __print_flags(REC->gfp_flags, \"|\", {( unsigned long)(((((((( gfp_t)(((((1UL))) << (10))|((((1UL))) << (11)))) | (( gfp_t)((((1UL))) << (6))) | (( gfp_t)((((1UL))) << (7))) | (( gfp_t)((((1UL))) << (20)))) | (( gfp_t)((((1UL))) << (1)))) | (( gfp_t)((((1UL))) << (3))) | (( gfp_t)0)) | (( gfp_t)((((1UL))) << (18))) | (( gfp_t)((((1UL))) << (19))) | (( gfp_t)((((1UL))) << (13)))) & ~(( gfp_t)(((((1UL))) << (10))|((((1UL))) << (11))))) | (( gfp_t)((((1UL))) << (10)))), \"GFP_TRANSHUGE\"}, {( unsigned long)((((((( gfp_t)(((((1UL))) << (10))|((((1UL))) << (11)))) | (( gfp_t)((((1UL))) << (6))) | (( gfp_t)((((1UL))) << (7))) | (( gfp_t)((((1UL))) << (20)))) | (( gfp_t)((((1UL))) << (1)))) | (( gfp_t)((((1UL))) << (3))) | (( gfp_t)0)) | (( gfp_t)((((1UL))) << (18))) | (( gfp_t)((((1UL))) << (19))) | (( gfp_t)((((1UL))) << (13)))) & ~(( gfp_t)(((((1UL))) << (10))|((((1UL))) << (11))))), \"GFP_TRANSHUGE_LIGHT\"}, {( unsigned long)((((( gfp_t)(((((1UL))) << (10))|((((1UL))) << (11)))) | (( gfp_t)((((1UL))) << (6))) | (( gfp_t)((((1UL))) << (7))) | (( gfp_t)((((1UL))) << (20)))) | (( gfp_t)((((1UL))) << (1)))) | (( gfp_t)((((1UL))) << (3))) | (( gfp_t)0)), \"GFP_HIGHUSER_MOVABLE\"}, {( unsigned long)(((( gfp_t)(((((1UL))) << (10))|((((1UL))) << (11)))) | (( gfp_t)((((1UL))) << (6))) | (( gfp_t)((((1UL))) << (7))) | (( gfp_t)((((1UL))) << (20)))) | (( gfp_t)((((1UL))) << (1)))), \"GFP_HIGHUSER\"}, {( unsigned long)((( gfp_t)(((((1UL))) << (10))|((((1UL))) << (11)))) | (( gfp_t)((((1UL))) << (6))) | (( gfp_t)((((1UL))) << (7))) | (( gfp_t)((((1UL))) << (20)))), \"GFP_USER\"}, {( unsigned long)(((( gfp_t)(((((1UL))) << (10))|((((1UL))) << (11)))) | (( gfp_t)((((1UL))) << (6))) | (( gfp_t)((((1UL))) << (7)))) | (( gfp_t)((((1UL))) << (22)))), \"GFP_KERNEL_ACCOUNT\"}, {( unsigned long)((( gfp_t)(((((1UL))) << (10))|((((1UL))) << (11)))) | (( gfp_t)((((1UL))) << (6))) | (( gfp_t)((((1UL))) << (7)))), \"GFP_KERNEL\"}, {( unsigned long)((( gfp_t)(((((1UL))) << (10))|((((1UL))) << (11)))) | (( gfp_t)((((1UL))) << (6)))), \"GFP_NOFS\"}, {( unsigned long)((( gfp_t)((((1UL))) << (5)))|(( gfp_t)((((1UL))) << (11)))), \"GFP_ATOMIC\"}, {( unsigned long)((( gfp_t)(((((1UL))) << (10))|((((1UL))) << (11))))), \"GFP_NOIO\"}, {( unsigned long)((( gfp_t)((((1UL))) << (11))) | (( gfp_t)((((1UL))) << (13)))), \"GFP_NOWAIT\"}, {( unsigned long)(( gfp_t)((((1UL))) << (0))), \"GFP_DMA\"}, {( unsigned long)(( gfp_t)((((1UL))) << (2))), \"GFP_DMA32\"}, {( unsigned long)(( gfp_t)(((((1UL))) << (10))|((((1UL))) << (11)))), \"__GFP_RECLAIM\"}, {( unsigned long)(( gfp_t)((((1UL))) << (0))), \"__GFP_DMA\"}, {( unsigned long)(( gfp_t)((((1UL))) << (1))), \"__GFP_HIGHMEM\"}, {( unsigned long)(( gfp_t)((((1UL))) << (2))), \"__GFP_DMA32\"}, {( unsigned long)(( gfp_t)((((1UL))) << (3))), \"__GFP_MOVABLE\"}, {( unsigned long)(( gfp_t)((((1UL))) << (4))), \"__GFP_RECLAIMABLE\"}, {( unsigned long)(( gfp_t)((((1UL))) << (5))), \"__GFP_HIGH\"}, {( unsigned long)(( gfp_t)((((1UL))) << (6))), \"__GFP_IO\"}, {( unsigned long)(( gfp_t)((((1UL))) << (7))), \"__GFP_FS\"}, {( unsigned long)(( gfp_t)((((1UL))) << (8))), \"__GFP_ZERO\"}, {( unsigned long)(( gfp_t)((((1UL))) << (10))), \"__GFP_DIRECT_RECLAIM\"}, {( unsigned long)(( gfp_t)((((1UL))) << (11))), \"__GFP_KSWAPD_RECLAIM\"}, {( unsigned long)(( gfp_t)((((1UL))) << (12))), \"__GFP_WRITE\"}, {( unsigned long)(( gfp_t)((((1UL))) << (13))), \"__GFP_NOWARN\"}, {( unsigned long)(( gfp_t)((((1UL))) << (14))), \"__GFP_RETRY_MAYFAIL\"}, {( unsigned long)(( gfp_t)((((1UL))) << (15))), \"__GFP_NOFAIL\"}, {( unsigned long)(( gfp_t)((((1UL))) << (16))), \"__GFP_NORETRY\"}, {( unsigned long)(( gfp_t)((((1UL))) << (17))), \"__GFP_MEMALLOC\"}, {( unsigned long)(( gfp_t)((((1UL))) << (18))), \"__GFP_COMP\"}, {( unsigned long)(( gfp_t)((((1UL))) << (19))), \"__GFP_NOMEMALLOC\"}, {( unsigned long)(( gfp_t)((((1UL))) << (20))), \"__GFP_HARDWALL\"}, {( unsigned long)(( gfp_t)((((1UL))) << (21))), \"__GFP_THISNODE\"}, {( unsigned long)(( gfp_t)((((1UL))) << (22))), \"__GFP_ACCOUNT\"}, {( unsigned long)(( gfp_t)((((1UL))) << (23))), \"__GFP_ZEROTAGS\"}, {( unsigned long)(( gfp_t)((((1UL))) << (24))), \"__GFP_NO_OBJ_EXT\"}, { 0, ((void *)0) } ) : \"none\", REC->node, (1 && (REC->gfp_flags & ( unsigned long)(( gfp_t)((((1UL))) << (22))))) ? \"true\" : \"false\"",
    fields: "\tfield:unsigned long call_site;\toffset:8;\tsize:8;\tsigned:0;\n\
             \tfield:const void *ptr;\toffset:16;\tsize:8;\tsigned:0;\n\
             \tfield:size_t bytes_req;\toffset:24;\tsize:8;\tsigned:0;\n\
             \tfield:size_t bytes_alloc;\toffset:32;\tsize:8;\tsigned:0;\n\
             \tfield:unsigned long gfp_flags;\toffset:40;\tsize:8;\tsigned:0;\n\
             \tfield:int node;\toffset:48;\tsize:4;\tsigned:1;\n",
    call (call_site: u64, ptr: u64, bytes_req: u64, bytes_alloc: u64,
          gfp_flags: u64, node: i32, accounted: bool),
    render |args: &[u64; TRACE_ARGS]| {
        format!(
            "call_site={} ptr={:#x} bytes_req={} bytes_alloc={} gfp_flags={} node={} accounted={}",
            ps_render(args[0]),
            args[1],
            args[2],
            args[3],
            gfp_flags_str(args[4]),
            args[5] as i32,
            if args[6] != 0 { "true" } else { "false" },
        )
    };

    /// kmem:kfree — one kernel heap free, same origin discipline. The
    /// reference host's events/kmem/kfree, field list and print fmt
    /// verbatim: call_site and ptr, nothing else.
    EVENT_KFREE / kfree_fmt / trace_kfree : "kmem", "kfree", 24,
    print: "\"call_site=%pS ptr=%p\", (void *)REC->call_site, REC->ptr",
    fields: "\tfield:unsigned long call_site;\toffset:8;\tsize:8;\tsigned:0;\n\
             \tfield:const void *ptr;\toffset:16;\tsize:8;\tsigned:0;\n",
    call (call_site: u64, ptr: u64),
    render |args: &[u64; TRACE_ARGS]| {
        format!("call_site={} ptr={:#x}", ps_render(args[0]), args[1])
    };
}

/// The trace_marker pseudo-event. NOT in EVENTS on purpose: a marker
/// is not an enable-gated tracepoint (the reference records marker
/// writes whenever tracing is on, with no event switch), so it does
/// not appear in available_events or the events tree. The reference
/// renders the line's FUNCTION column with the writing function's
/// symbol (tracing_mark_write); Rux names the event exactly that.
/// The id is deliberately out of the table's range.
pub static EVENT_TRACE_MARKER: TraceEvent = TraceEvent {
    group: "ftrace",
    name: "tracing_mark_write",
    id: 0xfff,
    print_fmt: "\"%s\", REC->buf",
    fields: "\tfield:char buf[];\toffset:8;\tsize:0;\tsigned:0;\n",
    enabled: AtomicBool::new(true),
    fmt_args: fmt_trace_marker,
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
    /// How many records a trace_pipe reader has consumed (the stats
    /// file's "read events" column).
    pub read_events: AtomicU64,
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
            read_events: AtomicU64::new(0),
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

/// Drop one CPU's buffered records (per_cpu/cpuN/trace's write).
pub fn clear_cpu(cpu: usize) {
    if cpu < RINGS.len() {
        RINGS[cpu].head.store(0, Ordering::Relaxed);
        RINGS[cpu].count.store(0, Ordering::Relaxed);
    }
}

/// Record a trace_marker write (the write-only tracefs file): markers
/// are gated by tracing_on alone, never by an event switch — the
/// reference's marker path has no enable file. The text is capped at
/// what the argument slots carry (80 bytes).
pub fn record_marker(text: &[u8]) -> bool {
    if !TRACING_ON.load(Ordering::Relaxed) {
        return false;
    }
    let cpu = crate::arch::cpu_id() as usize;
    if cpu >= crate::config::MAX_CPUS {
        return false;
    }
    let bytes = &text[..text.len().min(TRACE_ARGS * 8)];
    let mut args = [0u64; TRACE_ARGS];
    for (i, word) in args.iter_mut().enumerate() {
        let off = i * 8;
        if off >= bytes.len() {
            break;
        }
        let end = (off + 8).min(bytes.len());
        let mut chunk = [0u8; 8];
        chunk[..end - off].copy_from_slice(&bytes[off..end]);
        *word = u64::from_le_bytes(chunk);
    }
    record(&EVENT_TRACE_MARKER, args)
}

/// The records of every CPU, in sequence order, without consuming
/// them (the `trace` file).
pub fn snapshot() -> Vec<TraceRecord> {
    let mut out: Vec<TraceRecord> = Vec::new();
    for ring in RINGS.iter() {
        collect_ring(ring, &mut out);
    }
    out.sort_by_key(|r| r.seq);
    out
}

/// Take and consume the buffered records (the `trace_pipe` file: a
/// read spools the records out of the buffer).
pub fn drain() -> Vec<TraceRecord> {
    let mut out: Vec<TraceRecord> = Vec::new();
    for ring in RINGS.iter() {
        let taken = collect_ring(ring, &mut out) as u64;
        // Consume: a racing writer may land one record into the slots
        // we just read; it is dropped with the reset — the same
        // spool-and-reset contract the reference host's pipe reader has.
        ring.head.store(0, Ordering::Release);
        ring.count.store(0, Ordering::Release);
        ring.read_events.fetch_add(taken, Ordering::Relaxed);
    }
    out.sort_by_key(|r| r.seq);
    out
}

/// One CPU's buffered records, unconsumed (per_cpu/cpuN/trace).
pub fn snapshot_cpu(cpu: usize) -> Vec<TraceRecord> {
    let mut out: Vec<TraceRecord> = Vec::new();
    if cpu < RINGS.len() {
        collect_ring(&RINGS[cpu], &mut out);
    }
    out
}

/// Take and consume one CPU's records (per_cpu/cpuN/trace_pipe).
pub fn drain_cpu(cpu: usize) -> Vec<TraceRecord> {
    let mut out: Vec<TraceRecord> = Vec::new();
    if cpu < RINGS.len() {
        let ring = &RINGS[cpu];
        collect_ring(ring, &mut out);
        ring.head.store(0, Ordering::Release);
        ring.count.store(0, Ordering::Release);
        ring.read_events
            .fetch_add(out.len() as u64, Ordering::Relaxed);
    }
    out
}

/// The per_cpu/cpuN/stats file text, in the reference's eight-field
/// shape. Mapping: entries = buffered count; overrun = records
/// overwritten by newer ones (pushes minus live count); commit
/// overrun = 0 (no commit-overrun concept in this ring); bytes =
/// entries x the fixed record size; oldest event ts / now ts in the
/// reference's seconds.6-digit shape; dropped events = 0 (nothing but
/// overwrites drops records here); read events = the pipe readers'
/// consumption.
pub fn cpu_stats(cpu: usize) -> String {
    if cpu >= RINGS.len() {
        return String::new();
    }
    let ring = &RINGS[cpu];
    let count = ring.count.load(Ordering::Acquire).min(RING_CAPACITY);
    let head = ring.head.load(Ordering::Acquire);
    let overrun = head.saturating_sub(count);
    let record_bytes = core::mem::size_of::<TraceRecord>();
    let start = (head + RING_CAPACITY - count) % RING_CAPACITY;
    let oldest = (0..count)
        .map(|i| ring.records[(start + i) % RING_CAPACITY].ts_ns)
        .min()
        .unwrap_or(0);
    let now = crate::drivers::timer::monotonic_ns_since_boot();
    let secs = |ns: u64| format!("{}.{:06}", ns / 1_000_000_000, (ns % 1_000_000_000) / 1000);
    format!(
        "entries: {}\noverrun: {}\ncommit overrun: 0\nbytes: {}\noldest event ts: {}\nnow ts: {}\ndropped events: 0\nread events: {}\n",
        count,
        overrun,
        count * record_bytes,
        secs(oldest),
        secs(now),
        ring.read_events.load(Ordering::Relaxed),
    )
}

/// Copy one ring's live records out in ring order; returns how many
/// were taken.
fn collect_ring(ring: &TraceRing, out: &mut Vec<TraceRecord>) -> usize {
    let count = ring.count.load(Ordering::Acquire).min(RING_CAPACITY);
    let head = ring.head.load(Ordering::Acquire);
    let start = (head + RING_CAPACITY - count) % RING_CAPACITY;
    for i in 0..count {
        out.push(ring.records[(start + i) % RING_CAPACITY]);
    }
    count
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
    let event = if record.event_id == EVENT_TRACE_MARKER.id {
        Some(&EVENT_TRACE_MARKER)
    } else {
        EVENTS.iter().find(|e| e.id == record.event_id).copied()
    };
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

/// per_cpu/cpuN/trace: the same header shape with that CPU's counts,
/// then that CPU's records only.
pub fn render_trace_cpu(cpu: usize) -> String {
    let mut out = String::new();
    if cpu < RINGS.len() {
        let ring = &RINGS[cpu];
        let count = ring.count.load(Ordering::Acquire).min(RING_CAPACITY);
        let written = ring.head.load(Ordering::Acquire);
        out.push_str("# tracer: nop\n");
        out.push_str("#\n");
        out.push_str(&format!(
            "# entries-in-buffer/entries-written: {}/{}   #P:{}\n",
            count,
            written,
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
        for record in snapshot_cpu(cpu).iter() {
            out.push_str(&render_record(record));
        }
    }
    out
}

/// per_cpu/cpuN/trace_pipe: consume that CPU's records, no header.
pub fn render_pipe_cpu(cpu: usize) -> String {
    let mut out = String::new();
    for record in drain_cpu(cpu).iter() {
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

/// The one-slot ASCII tag packed by `PackArg for &str` back to text (the
/// rwbs column of the block events).
fn ascii_from_word(word: u64) -> String {
    let bytes = word.to_le_bytes();
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// The ioprio class name — the reference's __print_symbolic table over
/// the class bits (IOPRIO_CLASS_NONE..INVALID); a value outside the
/// table prints as hex, exactly as __print_symbolic does.
fn ioprio_class(ioprio: u16) -> String {
    match (ioprio >> 13) & 7 {
        0 => "none".to_string(),
        1 => "rt".to_string(),
        2 => "be".to_string(),
        3 => "idle".to_string(),
        4 => "invalid".to_string(),
        v => format!("{:#x}", v),
    }
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

/// The trace_marker record's text: the argument slots unpacked back
/// to bytes (the marker writes zero-fill the tail).
fn fmt_trace_marker(args: &[u64; TRACE_ARGS]) -> String {
    let mut bytes = [0u8; TRACE_ARGS * 8];
    for (i, word) in args.iter().enumerate() {
        bytes[i * 8..i * 8 + 8].copy_from_slice(&word.to_le_bytes());
    }
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// `%ps`/`%pS` rendering — the symbol table resolves the address to
/// name+off/size and falls back to the raw hex exactly as the
/// reference's %ps does when no symbol covers the address. Shared by
/// the page_fault pair and the kmem origin column.
pub fn ps_render(addr: u64) -> String {
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
}

/// gfp_flags rendered the print fmt's way: 0 is "none", otherwise the
/// named mask (the reference's __print_flags; Rux sites pass
/// GFP_KERNEL-class masks today, and the table covers the reference's
/// common set).
fn gfp_flags_str(gfp: u64) -> &'static str {
    match gfp {
        0 => "none",
        0xcc0 => "GFP_KERNEL",
        0xca0 => "GFP_NOFS",
        0xc20 => "GFP_ATOMIC",
        0xc00 => "GFP_NOIO",
        0x220 => "GFP_HIGHUSER_MOVABLE",
        0x200 => "GFP_HIGHUSER",
        0x140 => "GFP_USER",
        _ => "GFP_KERNEL",
    }
}

/// Pick the kmalloc/kfree `call_site` out of a frame walk done at the
/// GlobalAlloc boundary: the first frame ABOVE the allocation plumbing.
/// The reference's `_RET_IP_` names the caller of kmalloc — which is
/// inline at the interesting site — while a Rust GlobalAlloc hop adds
/// TWO OR THREE shim frames (current_frame_pointer's own frame, the
/// GlobalAlloc impl, `__rust_alloc`, the alloc crate's wrapper), and
/// the shims are not always in the symbol table (an address inside
/// `__rust_alloc` then resolves to the closest-below symbol — the
/// GlobalAlloc impl itself). The skip table names every frame the walk
/// can legitimately pass through while still inside the allocator; the
/// first frame resolving elsewhere is the allocating kernel function —
/// the address the heap-leak hunt needs. Falls back to the first
/// non-zero frame when nothing resolves (early boot, no table).
pub fn alloc_call_site(frames: &[u64]) -> u64 {
    /// Is a resolved name part of the allocation plumbing the walk must
    /// climb past?
    fn is_plumbing(name: &str) -> bool {
        name.contains("mm::buddy_allocator")      // the GlobalAlloc impl + the trace helpers
            || name.contains("mm::small_alloc")   // the packing layer
            || name.contains("dfx::memwatch")     // the walker itself
            || name.contains("dfx::backtrace")    // current_frame_pointer
            // The compiler shims demangle inside the __rustc module
            // namespace ("__rustc::__rust_alloc+0x.."): substring, not
            // prefix, or the origin lands one frame too low.
            || name.contains("__rust_alloc")
            || name.contains("__rust_dealloc")
            || name.contains("__rust_realloc")
            // The WHOLE alloc crate is allocation machinery (raw_vec's
            // grow/deallocate, vec's push/reserve, boxed's new — and
            // their trait impls render as "<alloc::string::String as
            // ...>", so a starts_with test misses them). No kernel
            // allocating site lives there; stopping early would name
            // the growth, not the caller.
            || name.contains("alloc::alloc::")
            || name.contains("alloc::raw_vec")
            || name.contains("alloc::vec")
            || name.contains("alloc::string")
            || name.contains("alloc::boxed")
            || name.contains("alloc::fmt")
            || name.contains("alloc::collections")
            || name.contains("alloc::sync")
    }

    // One pass, two candidates: the best origin is a frame in the
    // kernel's own rux:: namespace (trait impls render as "<rux::...")
    // that is not itself plumbing — the GlobalAlloc impl IS a rux::
    // frame, so the plumbing test runs first. A non-rux frame that is
    // not plumbing (early boot, foreign code) serves as the fallback
    // when no kernel frame is within the walk.
    let mut fallback = 0u64;
    for &f in frames {
        if f == 0 {
            break;
        }
        let (name, is_rux) = match crate::dfx::ksyms::lookup(f) {
            Some((name, _, _)) => {
                let rux = name.starts_with("rux::") || name.starts_with("<rux::");
                (name, rux)
            }
            None => ("", false),
        };
        if !name.is_empty() && is_plumbing(name) {
            continue;
        }
        if is_rux {
            return f;
        }
        if fallback == 0 {
            fallback = f;
        }
    }
    if fallback != 0 {
        return fallback;
    }
    frames.iter().copied().find(|&f| f != 0).unwrap_or(0)
}

/// page_fault_user / page_fault_kernel: address, ip, error_code. The
/// `%ps` rendering is symbolic — the symbol table resolves the address
/// to name+off/size and falls back to the raw hex exactly as the
/// reference's %ps does when no symbol covers the address.
fn fmt_page_fault(args: &[u64; TRACE_ARGS]) -> String {
    format!(
        "address={} ip={} error_code=0x{:x}",
        ps_render(args[0]),
        ps_render(args[1]),
        args[2]
    )
}
