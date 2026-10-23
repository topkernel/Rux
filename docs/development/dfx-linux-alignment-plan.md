# DFX Alignment with Linux — Gap Analysis and Phased Plan

Date: 2026-10-24
Status: analysis complete; Tranche 1 implemented (see §5)
Scope: `kernel/src/dfx/` plus the user-visible debug surfaces
(`/proc`, `/sys`, printk, crash paths).

Design rule for this work: **every surface follows Linux**. Where Linux
has a facility — debugfs, `/proc/sysrq-trigger`, `/proc/kallsyms`,
`/proc/stat` — Rux uses that facility, with that name, that file format
and that command semantics. Rux does not invent parallel interfaces.

## 1. Why this document exists

Rux has grown a large but entirely **console-only** DFX layer: 20
modules, 29 boot-time switches, two watchdogs, a printk ring buffer, and
a pile of forensic printers. Every one of those instruments prints to
the serial console and nothing else. The kernel cannot be observed from
userspace, and the instruments cannot be armed or disarmed while the
system runs.

That has a concrete cost. The project debugs functional problems by
booting the kernel with a different `dfx=` command line for every hunt,
and cannot answer performance questions at all because the kernel exposes
no per-CPU time accounting, no VM statistics, no scheduler statistics and
no per-process context-switch counts.

This document states the gap against Linux, orders it by what unblocks
diagnosis first, and fixes the order of work.

## 2. What Rux already has (the asset inventory)

### 2.1 The `dfx=` instrument family

`kernel/src/dfx/switches.rs` holds 29 switches; before Tranche 1 every
one of them was **boot-time only** (`init_from_cmdline()` at
`switches.rs:347` was the only writer; `set()` had no callers).

| Module | Lines | Purpose | Cost when armed |
|---|---|---|---|
| `dfx/taskdump.rs` | 742 | all-task state snapshot (sched state, claims, runqueue links, optional backtrace) | on demand |
| `dfx/ledger.rs` | 491 | physical page census + zone counter reconciliation | on demand + 30 s series |
| `dfx/memwatch.rs` | 1066 | heap alloc/free size classes, call-site histograms | per-alloc counters |
| `dfx/scribble.rs` | 1658 | byte-level canary over live `Task` control words (x86_64) | per-switch + per-tick |
| `dfx/syscost.rs` | 696 | whole-syscall cost table + kernel-rip histogram (perf-audit line) | on demand |
| `dfx/wildwatch.rs` | 479 | RO-guard tripwire on the hpet quiet-statics cluster (x86_64) | arm-time + 1 Hz verify |
| `dfx/metawatch.rs` | 157 | buddy free-list vs bitmap cross-check | 10 s scan |
| `dfx/r83hunt.rs` | 566 | task-header corruption hunt | per-tick audit |
| `dfx/softlockup.rs` | 222 | per-CPU 10 s no-schedule detector | per-tick |
| `dfx/hung_task.rs` | 286 | khungtaskd: D-state > 120 s | 60 s scan |
| `dfx/backtrace.rs` | 341 | frame-pointer stack walk + register/CSR dump | on demand |
| `dfx/bug.rs` | 98 | WARN / BUG cut-here banners | on demand |
| `dfx/driftwatch.rs` | 130 | zero-put / free-refusal attribution | counters |
| `dfx/monowatch.rs` | 137 | CLOCK_MONOTONIC jump tripwire | 1 Hz |
| `dfx/taint.rs` | 108 | taint bitmask | — |
| `dfx/hexdump.rs`, `dfx/sbi_debug.rs` | — | **dead code** (zero callers) | — |

Triggers available before Tranche 1: UART magic `DUMP!` and `LEDGER!`
(`console.rs:558`, `console.rs:577`), the spinlock-deadlock watchdog
(`sync/spinlock.rs:236`), and a 30 s periodic timer (`timer.rs:720`).

### 2.2 What is missing entirely

There is **no** ftrace, **no** tracefs, **no** debugfs, **no**
perf_event subsystem (`sys_perf_event_open` returns `ENOSYS`,
`syscall/process.rs`), **no** kprobe/uprobe, **no** eBPF tracing
(`sys_bpf` returns `ENOSYS`; the only BPF in the tree is the
classic-BPF seccomp filter), **no** `/proc/kallsyms`, **no** runtime
symbolization, **no** SysRq facility, **no** KASAN/UBSAN/Kfence, **no**
lockdep/lockstat, **no** PSI.

### 2.3 Absent `/proc` surfaces that matter for diagnosis

`/proc/stat`, `/proc/vmstat`, `/proc/schedstat`, `/proc/sched`,
`/proc/softirqs`, `/proc/timer_list`, `/proc/buddyinfo`,
`/proc/pagetypeinfo`, `/proc/slabinfo`, `/proc/locks`,
`/proc/diskstats`, `/proc/devices`, `/proc/crypto`, `/proc/keys`,
`/proc/console`, `/proc/thread_info`, `/proc/sched_debug`,
`/proc/pressure/*`, `/proc/sysrq-trigger`, `/proc/kallsyms`.

### 2.4 Fidelity defects in surfaces that already exist

These are cheap to fix and actively mislead:

- `/proc/sys/kernel/tainted` was hardcoded to `0`
  (`fs/procfs/sysctl.rs`) although `dfx/taint.rs` maintained a real
  mask that WARN/BUG set. **Fixed in Tranche 1** — Linux's format is
  numeric, and the value is now the live mask.
- The taint letters in `dfx/taint.rs` were not Linux's
  (`b"GFfUvPDdCAcNIOsl"`). **Fixed in Tranche 1** — Linux's set is
  `P F S R M B U D A W C I O E L K`, rendered as a 16-column line with
  `G` in the first column when the proprietary-module bit is clear.
- `/proc/kmsg` uses the dmesg header format while `/dev/kmsg` uses
  Linux's `level,seq,timestamp_us,-;` — the two disagree.
- `/proc/[pid]/stat` reports `vsize` in bytes where Linux reports pages,
  and leaves fields 7–13, 18–22, 24–30, 32–53 as literal `0`.
- `/proc/interrupts` hardcodes the IRQ list to `1..=15` + `32..=47`
  instead of walking the irq descriptor registry.
- `readdir` on `/proc/[pid]` omits `smaps` and `task`.
- `printk::persistent_log_init()` is **commented out** at `main.rs`, so
  the persistent log the DFX plan claims exists never runs.
- The panic handler halts in a `wfi` loop; there is no `panic=` timeout
  policy and no reboot-after-panic.
- There is no non-fatal "Oops" path: every fault goes straight to
  `panic!`.

## 3. Linux DFX capability matrix

Legend: **OK** = present and Linux-shaped · **PART** = present but
incomplete or non-Linux shape · **--** = absent.

| Family | Linux facility | Rux | Gap |
|---|---|---|---|
| Logging | `printk` ring buffer, log levels, `/dev/kmsg` | PART | `/proc/kmsg` format wrong; persistent log disabled |
| Logging | `dmesg -w`, `syslog(2)`, `console_loglevel` | PART | no runtime console level knob from userspace |
| Crash | panic handler, `Call Trace:` | PART | header case differs, frames never symbolized |
| Crash | `Oops` (non-fatal) | -- | every fault is fatal |
| Crash | `panic=` timeout / reboot | -- | halt forever |
| Crash | pstore / ramoops | -- | empty `/sys/fs/pstore` skeleton only |
| Crash | taint flags | OK | Tranche 1: exported, Linux letters |
| Control | SysRq / `/proc/sysrq-trigger` | OK | Tranche 1: upstream command set |
| Control | debugfs `/sys/kernel/debug` | OK | Tranche 1: new filesystem |
| Control | tracefs `/sys/kernel/tracing` | -- | absent |
| Control | ftrace / kprobe / uprobe / eBPF / perf | -- | absent (`ENOSYS`) |
| Control | runtime-switchable knobs | OK | Tranche 1: debugfs knobs |
| Symbols | `/proc/kallsyms` | -- | absent |
| Symbols | runtime backtrace symbolization | -- | raw PCs only |
| Symbols | ORC / DWARF unwinder | -- | frame-pointer walk only, 32 frames |
| Perf | `/proc/stat` | -- | absent: no per-CPU time accounting at all |
| Perf | `/proc/vmstat` | -- | absent |
| Perf | `/proc/schedstat`, `/proc/sched` | -- | absent |
| Perf | `/proc/softirqs` | -- | absent |
| Perf | `/proc/[pid]/stat` ctxt switches | PART | fields present but always `0` |
| Perf | PSI `/proc/pressure/*` | -- | absent |
| Perf | `/proc/buddyinfo`, `/proc/pagetypeinfo`, `/proc/slabinfo` | -- | absent (`print_buddyinfo` is dead code) |
| Perf | `/proc/diskstats`, `/proc/timer_list`, `/proc/lockstat` | -- | absent |
| Memory | KASAN / UBSAN / Kfence / slub debug | -- | absent (Rux substitutes are bespoke) |
| Memory | page poisoning on free | -- | absent |
| Locking | lockdep (lock-class graph, cycle detect) | -- | absent |
| Locking | lock owner tracking | PART | compile-time feature `dfx-lock-owner` only |
| Locking | soft lockup / hung task | OK | present and working |
| Locking | RCU stall detector | -- | absent |
| Storage | `/proc/diskstats`, `/proc/partitions` | PART | partitions present, diskstats absent |
| Storage | `blktrace`, `scsi_debug` | -- | absent |
| Network | `/proc/net/*` | OK | present, Linux column layouts |
| Network | `netstat -s` counters, `sockstat` | PART | `sockstat` present, error counters absent |
| Time | clocksource exposure, monotonic sanity | PART | `monowatch` exists but console-only |
| Power | `/sys/power/state` | OK | present |
| Container | cgroup v2 | PART | cpu/memory/pids only; no cgroup v1 `memory` mount |
| Core | kgdb / kdb | -- | absent |
| Core | kdump / kexec | -- | absent |

## 4. Phased plan

Ordering rule: a tranche must be usable by the *next* tranche. Live
control before tracing, counters before profiling, symbols before
symbolized traces.

### Tranche 1 — live control surface (implemented)

Goal: every existing instrument becomes armable and observable from
userspace, without a reboot, through the Linux interfaces.

1. **debugfs** (`kernel/src/fs/debugfs.rs`): a new filesystem mounted at
   `/sys/kernel/debug`, exactly like Linux. The mount point is a real
   sysfs directory (`/sys/kernel/debug`), so sysfs owns the directory
   and debugfs is mounted over it — the Linux arrangement.
2. **Switch knobs** at `/sys/kernel/debug/dfx/<switch>`: one file per
   switch, named after its `dfx=` spelling. Linux's `debugfs_create_bool`
   convention — the file reads `Y`/`N`, a write of `Y`/`y`/`1` arms the
   instrument and `N`/`n`/`0` disarms it, anything else is `EINVAL`.
   The switch table became a single source of truth: one macro
   generates the enum→name mapping, the `ALL` enumeration and the knob
   files, so a new switch cannot drift from its control file.
3. **`/proc/sysrq-trigger`**: the upstream SysRq contract — only the
   first character is handled unless the write starts with `_` (then
   every following character is a command), commands are case-sensitive,
   `h` and any unlisted key print the help. The implemented commands
   are the upstream ones (`b c d e f h i k l m n o p q r s t u v w x y z
   0-9 R`); the ones Rux cannot honour print an explicit "not supported"
   line instead of silently doing nothing.
4. **Taint fidelity**: `/proc/sys/kernel/tainted` carries the live
   bitmask in Linux's numeric format, and the "Tainted:" line uses
   Linux's letters and 16-column layout.

Acceptance (verified in the guest, `test/dfx-smoke.sh`): the knob
directory lists all 29 switches; a knob reads `N`, arms to `Y` on
`echo 1 > …`, disarms on `echo 0 > …`; `/proc/sysrq-trigger` runs the
task dump and the help; `/proc/sys/kernel/tainted` reports the live
mask; `/sys/kernel` still lists `uevent`, `uevent_helper`,
`uevent_seqnum` and the new `debug` mount point.

### Tranche 2 — performance counters (part 1 done)

Goal: answer "where did the time go" from inside the guest.

**Part 1 — implemented.** `/proc/stat` in Linux's layout
(`kernel/src/fs/procfs/stat.rs`): the aggregate `cpu` line, one line per
CPU, `intr`, `ctxt`, `btime`, `processes`, `procs_running`,
`procs_blocked`. The timer tick charges one jiffy to whatever was
running — idle for the idle task, user for a task hit in user mode,
system otherwise (a kernel thread is system time, not idle time). The
per-task context-switch counters (Linux `nvcsw`/`nivcsw`) are counted in
the scheduler's `context_switch` — a task that went to sleep blocked
(voluntary), a task still runnable was preempted (involuntary) — and
surface in `/proc/[pid]/status`. `/proc/[pid]/stat` now has Linux's 52
fields (it emitted 54) and fills priority, nice, num_threads, rss (in
pages), processor and policy next to the utime/stime the timer IRQ
already charged.

**Part 2 — implemented.** `/proc/vmstat` (Linux key names, the page
counters live in `kernel/src/mm/vmstat.rs`, the memory manager owning
them like Linux's mm/vmstat.c), `/proc/buddyinfo` (free pages per order
per zone, from the zone allocator's per-order free lists),
`/proc/softirqs` (per-CPU per-vector raise counts, using the vector set
and names Rux's softirq module already uses) and `/proc/schedstat`
(version 17: timestamp plus the nine per-CPU fields, with real
`yld_count` and `sched_count`). `/proc/stat` also grew Linux's
aggregate `softirq` line.

**Part 3 — remaining.**

- `/proc/sched` (per-task runqueue view), `/proc/pagetypeinfo`,
  `/proc/locks`, `/proc/diskstats`, `/proc/timer_list`.
- Per-task minor/major fault counters (fields 10/12 of
  `/proc/[pid]/stat`) and `starttime` (field 22).
- `pgpgin`/`pgpgout` (block-I/O page counters) — Rux has a block layer
  but no page accounting in it yet.

Acceptance: `/proc/stat` totals track `/proc/uptime` within one tick per
CPU; `/proc/vmstat` counters are monotonic and reconcile against the
ledger census; a busy-loop guest shows non-zero `user` and rising
`voluntary_ctxt_switches`.

### Tranche 3 — symbols and symbolized traces

Goal: a backtrace that names its frames.

- Build-time symbol table: `nm` over the linked kernel ELF, filtered to
  text symbols, embedded as a compact table; the link pass runs twice
  (the table lives in `.rodata`, so `.text` addresses are stable between
  the two passes).
- `/proc/kallsyms` in Linux's format (addresses zeroed unless
  `kptr_restrict` allows).
- Runtime symbolization inside `dfx/backtrace.rs`: frames print
  `func+0xoff/0xsize` next to the raw address, so WARN/BUG/panic output
  becomes triageable without offline `addr2line`.
- Linux's `Call Trace:` capitalization and `?`/`0x` decoration.

Acceptance: a WARN in the kernel prints a frame with the Rust function
name; `/proc/kallsyms` lists the same names.

### Tranche 4 — tracepoint framework and tracefs

Goal: a general-purpose event facility to replace the ad-hoc rings.

- `tracepoint!` macro with a per-event id table, enabled/disabled at
  runtime.
- Per-CPU lockless ring buffers with a global sequence, drained by a
  reader.
- `/sys/kernel/tracing` (tracefs) with `available_tracers`, `trace`,
  `events/<group>/<event>/{enable,id,format}`, `trace_pipe` — the Linux
  layout, including the `tracing_on` master switch.
- Reuse the existing ring shapes as the first events: syscall entry/exit,
  scheduler pick/switch, page fault, block I/O completion, binder
  transaction.
- `trace-cmd`-style text output so the same tooling reads Rux and Linux.

Acceptance: enabling `sched:sched_switch` produces a switch trace
readable from a guest shell; the trace ring survives a 10-minute soak
without losing the first events.

### Tranche 5 — crash policy and persistent evidence

- `panic=` timeout (`panic=N`, `panic=o`, `panic=immediate`): reboot
  after N seconds instead of halting in `wfi` forever.
- Non-fatal `Oops` path for recoverable faults (Linux's `oops=panic`
  default off).
- pstore/ramoops-lite: keep the last panic record (registers + the tail
  of the ring buffer) in a reserved RAM area and expose it through
  `/sys/fs/pstore/` after the next boot.
- Re-enable `printk::persistent_log_init()` with a size cap, so the
  serial tail survives the reboot.

Acceptance: `panic=5` reboots the guest within five seconds of a
panic; the next boot shows the previous panic's registers.

### Tranche 6 — correctness and lock instrumentation

- lockdep-lite: per-lock-class acquisition order recording with a cycle
  report (`INFO: possible circular locking dependency detected`).
- `/proc/lockstat`: per-class contention counts.
- Page poisoning on free (buddy + heap) with a `PFN` poison pattern, and
  a `/proc/poison` view of poisoned pages.
- PSI-lite: `/proc/pressure/{cpu,io,memory}` from stall accounting.

### Tranche 7 — performance measurement infrastructure

(Depends on Tranches 2 and 4.) Per the project plan, this feeds the
microbench report: syscall latency per class, fork/exec, fault paths,
context switch, futex/pipe/binder round trip, TLB shootdown cost,
mmap/brk/mprotect, IO path — with the same QEMU configuration compared
against a Linux guest.

## 5. Explicit non-goals

- **eBPF**: the verifier and JIT are out of scope. A tracepoint + ring
  buffer + tracefs design covers the same diagnosis needs at a fraction
  of the complexity. Revisit only if a workload genuinely needs
  programmable filters.
- **kdump / kexec**: a second kernel is out of scope while the first
  kernel still has open correctness work.
- **kgdb / kdb**: the project already uses QEMU gdb stubs against the
  unstripped kernel image; the marginal value is low.
- **Upstreaming the bespoke instruments to Linux shape**: the Rux
  instruments (ledger, metawatch, wildwatch, scribble, syscost) are more
  specific than their Linux equivalents. They stay; the Linux-shaped
  surfaces are added alongside.

## 6. Verification method

Every tranche is verified in the guest, not by inspection:

- Build: `cargo build --target riscv64gc-unknown-none-elf` and
  `--target x86_64-unknown-none --no-default-features --features x86_64`.
- Boot the minimal rootfs and exercise the new `/proc` and `/sys`
  surfaces from the guest shell (`test/dfx-smoke.sh` drives the current
  Tranche).
- Confirm the LTP pass rate does not regress
  (`/home/william/rux-agents/ltp-r<N>/results/`).
- For every new file, compare the output format against a Linux guest
  running the same command, field by field.

## 7. File anchors

| Area | File |
|---|---|
| switch table | `kernel/src/dfx/switches.rs` |
| instrument modules | `kernel/src/dfx/*.rs` |
| debugfs | `kernel/src/fs/debugfs.rs` |
| procfs tree | `kernel/src/fs/procfs/mod.rs` |
| procfs generators | `kernel/src/fs/procfs/{meminfo,interrupts,loadavg,uptime,pid,sysctl,sysrq}.rs` |
| printk / kmsg | `kernel/src/printk.rs` |
| panic handler | `kernel/src/main.rs` |
| backtrace | `kernel/src/dfx/backtrace.rs` |
| page allocator | `kernel/src/mm/page_alloc.rs` |
| scheduler | `kernel/src/sched/sched.rs` |
| timer | `kernel/src/timer.rs` |
