# The debug subsystem — design

Status: DESIGN OF RECORD (2026-11-25, user directives of that day).
Scope: what the kernel provides for locating a failure, how those
facilities are organized, where each lives, and how the existing
single-failure instruments are folded into them or deleted.

Related documents. The kernel's overall structure is
docs/architecture/seed-core-design.md (§7a points here). How to use
the facilities is docs/guides/debugging.md. The record of retiring
individual instruments is docs/development/debug-retirement-review.md.

## 1. Directives (user, 2026-11-25)

- A failure is located by reading the log, or by running again under
  a debug build and reading the log — not by adding code for that one
  failure and digging again.
- The debug subsystem is reviewed against the reference kernel and
  brought in line with what it provides.
- Unneeded code is removed wherever it is: obsolete features and dead
  code, in the core and in the subsystem crates, not only printers.
  An instrument is not moved to a crate to keep it alive; it is
  replaced by an existing facility, folded into one, or deleted.
- Debug features are off in release builds (rule of 2026-11-19).

## 2. Why the method has been the other kind

The debug subsystem grew one instrument per hunt. Its files are named
for the failures they were written against — `driftwatch`, `lostpark`,
`schb`, `scmtap`, `spinwatch`, `mmprof`, `forkprof`, `hilogcensus`,
`allocwatch`, `wildwatch`, `leakorigin`, `scribble`. Each watches one
symptom, each is reached by the debug event that arms it, and none
answers a question it was not written for.

The general facilities exist and are less used than the specific
ones. Measured on 2026-11-25:

| | |
|---|---|
| trace points declared | 216, in 18 groups |
| of which in the hunt-named `rux` group | 58 |
| `debug/` files in the core | 35 |
| scattered instrument lines in the other subsystems | about 17,000 (§2c.3 of the structure document) |

The NOVMA-EXEC investigation of 2026-11-23 spent seven commits on
capture code. `raw_syscalls:sys_enter` was declared and wired the
whole time. Whether it would have answered the question is not
recorded; that it was not the first step is the fact this document
responds to.

## 3. State, measured

Each row was checked against the tree on 2026-11-25. "Here" is what
exists, not what is planned. Rows marked NOT CHECKED were not
verified and must be before any work relies on them.

| facility | for | reference kernel | here |
|---|---|---|---|
| log buffer with levels | what happened, in order | `printk`, `dmesg` | yes; a persistent log (`/var/log/kmsg`); some writers bypass it (§3.2) |
| trace points per subsystem | the sequence around a failure | `sched_switch`, `sched_wakeup`, `block_rq_issue`, `irq_handler_entry`, … | 216 points, 18 groups. `sched` 18, `irq` 4, `block` 4, `raw_syscalls` 4 (`sys_enter`, `sys_exit`), `ext4` 14, `binder` 10, `net` 10, `kmem` 8, `input` 8 |
| tracefs | control the points from user space | `/sys/kernel/tracing` | mounted at `/sys/kernel/tracing`; `tracing_on` exists. The rest of the control files: NOT CHECKED |
| dynamic debug | turn up detail in one place, no rebuild | `/sys/kernel/debug/dynamic_debug/control` | no |
| kprobes | a probe on any function, at run time | `/sys/kernel/tracing/kprobe_events` | no |
| function tracer | call sequence, per-function time | `function`, `function_graph` | no (`syscost` samples syscalls) |
| sysrq | a stuck system, from the keyboard | about 12 commands | 12 commands (`b c d e f i k l m n o p`) |
| task dump | who is stuck where | `sysrq-t`, `/proc/<pid>/stack` | `taskdump` |
| hung-task, soft-lockup detectors | a task or CPU making no progress | `khungtaskd` | `hung_task`, `softlockup` |
| hard-lockup watchdog | a CPU with interrupts off | NMI watchdog | no |
| RCU stall detection | a grace period that does not end | RCU stall warnings | `sync/hotpath.rs` (rcu section) |
| `WARN` / `BUG` with a stack, rate limited | a broken invariant, reported once | macros | no macros; 15 bare `dump_stack()` calls, 51 `debug_assert!` (no stack, absent in release) |
| slab and object debugging | a corrupted or leaked object | `/proc/slabinfo`, SLUB debug, `DEBUG_OBJECTS` | `/proc/slabinfo`; SLUB debug flags and object debugging NOT CHECKED |
| lockdep | a deadlock before it happens | `PROVE_LOCKING` | no |
| KASAN, KFENCE | out-of-bounds, use-after-free at run time | yes | no |
| crash dump | the state at a panic | `kdump`, vmcore | `pstore` keeps the log; no memory image |
| boot-time switches | arm an instrument from the command line | `debug=`, `trace_event=` | `trace_event=` for the points; 7 permanent `debug=` switches (`claiminject`, `oncpuinject`, `scribblepark`, `scribblesweep`, `sprobe`, `wildwatch`, and the valueless loglevel flag) |

### 3.1 Open defects in the trace points

- `exceptions:page_fault_user` and `exceptions:page_fault_kernel` are
  declared with no call site. The page-fault event exists by name only.
- 58 of the 216 points sit in the `rux` group, named for the session
  that needed them (`ams46`, `acqdiv`, `leakhunt`, `ohhunt`,
  `epollscan`).
- The `timer` group holds `itimer_arm` alone: no timer-expiry point.
- The reference's group-by-group list has not been compared with this
  one; which general points are undeclared is not yet known beyond
  the two above.

### 3.2 Debug code is not off in release

The rule of 2026-11-19 is that release builds carry no debug feature.
It holds for the raw console writers (`console::putchar_no_lock` is
a no-op without `debug_assertions`) and for little else.

- 1 of the 35 files under `core/src/debug/` has a release gate.
- In the release x86_64 kernel, counted in the symbol table:
  `syscost` 47 symbols, `scribble` 38, `taskdump` 28, `memwatch` 25,
  `allocwatch` 20, `leakorigin` 19, `backtrace` 16, `softlockup` 14,
  `wildwatch` 11.
- The taskdump funnel (`putc_brief` → `staged_putc`) has no gate by
  design: its comment says it is raw so that prints stay visible in
  release.

What this costs at run time has not been measured. An unarmed
instrument's cost is meant to be one branch; that is a claim in the
code, not a number.

### 3.3 One printer per hunt

Hand-written number-to-text printers and the print statements that
call them: 5,976 lines in the core, outside `tests/` and outside
`debug/`. `fn ph` is defined in 21 files, `fn pdec` in 7, `fn phex`
in 6. They write bytes to the console directly, so they bypass the
log buffer's levels and the release gate.

## 4. Target

A debug build carries the facilities of §5 and is the build the
method runs on. A release build carries none of them beyond what
§5.1 keeps.

```
kernel/
  core/src/
    printk.rs  console.rs            the log buffer and its writers (always present)
    trace/                           the trace-event framework (always present; a point
                                     costs one branch when unarmed)
    panic path                       always present
  debug/                            the debug subsystem crate:
                                     subsystem-independent instruments, each a
                                     facility of §5, none named for a failure
```

Rules:

- An instrument lives in the `debug` crate unless it must read core
  state through an unsafe edge; then its unsafe part is a function in
  the subsystem's `unsafe.rs` and the instrument is still in `debug`.
- No facility is named for the failure it was written against.
- A print in the core or a crate goes through `printk` or a trace
  point. No writer bypasses the log buffer.
- Every file in `debug/` and every instrument outside it is absent
  from a release build. A gate script asserts it (§6).
- Facilities are controlled from user space through the reference's
  interfaces — `tracefs`, `/proc/sys/kernel`, `sysrq` — so that
  tooling written for the reference works without change.

## 5. Facilities

### 5.1 Always present

The log buffer and its writers, the trace-event framework, the panic
path with its stack, `sysrq`, `/proc/kallsyms`. An armed trace point
costs a record; an unarmed one, one branch. The log buffer's levels
are the release build's only verbosity control.

### 5.2 Debug builds only

Per-subsystem trace points, dynamic debug, kprobes, the function
tracer, the hung-task and soft-lockup detectors, the hard-lockup
watchdog, slab and object debugging, lockdep, the crash dump, and
whatever of KASAN/KFENCE is feasible in Rust.

### 5.3 Mapping of the existing instruments

Each existing file and each scattered instrument gets one verdict,
recorded in docs/development/debug-retirement-review.md:

| verdict | when | example |
|---|---|---|
| replaced | a facility of §5 does the same job | the 5,976 printer lines, by dynamic debug |
| folded in | no facility covers it, but the observation is worth keeping | a watch on "task parked and not relinked" becomes a `sched` trace point and a check in user space |
| deleted | its hunt is closed | the instruments already retired on 2026-11-23 |
| kept as a facility | it is a detector in its own right | `softlockup`, `hung_task` |

## 6. Gates

- `scripts/debug_gate.py`: no file under `debug/` and no instrument
  marked as one appears in a release build (checked on the release
  ELF's symbol table); no writer outside `printk.rs`, `console.rs`
  and the panic path writes to the console directly; no trace point
  in the `rux` group; no trace point without a call site.
- The trace-point list is compared with a checked-in copy of the
  reference's list per group; a general point the reference has and
  this kernel lacks is reported.

## 7. Order of work

By how many investigations each item would have shortened.

1. **Wire the page-fault points** and compare the trace-point groups
   with the reference's; declare what is missing.
2. **Dynamic debug**: every diagnostic print becomes a site switched
   by a control file; the printers of §3.3 are replaced; the raw
   writers go through the log buffer.
3. **Release gate**: §6's first two checks land; the instruments
   that are debug-only are compiled out of release. What that costs
   or saves is measured and reported.
4. **Adjudicate the `rux` group and the scattered instruments**
   (§5.3): about 17,000 lines, subsystem by subsystem.
5. **tracefs control files** to the reference's set, so the points
   can be armed, filtered by pid, and read from user space the way
   the reference's tools expect.
6. **kprobes** on the core's functions.
7. **Function tracer, lockdep, hard-lockup watchdog**, then the rest
   of §5.2 as their own items.

Each item is a lane with its own gate, and each lands with the
number it changed: the line count it removed, or the investigation
step it made unnecessary.
