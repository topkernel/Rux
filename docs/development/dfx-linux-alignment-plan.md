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
| Crash | `Oops` (non-fatal) | OK | T5b: Linux-shaped report, task-death policy, pstore dmesg flavor |
| Crash | `panic=` timeout / reboot | -- | halt forever |
| Crash | pstore / ramoops | OK | Tranche 5: console-ramoops-0 (panic) + dmesg-ramoops-0 (oops), both arches |
| Crash | taint flags | OK | Tranche 1: exported, Linux letters |
| Control | SysRq / `/proc/sysrq-trigger` | OK | Tranche 1: upstream command set |
| Control | debugfs `/sys/kernel/debug` | OK | Tranche 1: new filesystem |
| Control | tracefs `/sys/kernel/tracing` | OK | Tranche 4: 16 events, tracepoint! table, soak-verified |
| Control | ftrace / kprobe / uprobe / eBPF / perf | PART | Tranche 4 tracepoints + nop tracer; probes/`perf` still `ENOSYS` |
| Control | runtime-switchable knobs | OK | Tranche 1: debugfs knobs |
| Symbols | `/proc/kallsyms` | OK | Tranche 3: full table, default-on (LAYOUT-3) |
| Symbols | runtime backtrace symbolization | OK | Tranche 3: name+off/size, `?` for non-text |
| Symbols | ORC / DWARF unwinder | -- | frame-pointer walk only, 32 frames |
| Perf | `/proc/stat` | -- | absent: no per-CPU time accounting at all |
| Perf | `/proc/vmstat` | -- | absent |
| Perf | `/proc/schedstat`, `/proc/sched` | PART | per-CPU schedstat rows + per-task /proc/[pid]/schedstat triple (T7 slice); /proc/sched still absent |
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
(version 17: jiffies timestamp plus the nine per-CPU fields, with real
`yld_count` (sched_yield() calls, as in kernel/sched/syscalls.c) and
`sched_count` in Linux's field positions). `/proc/stat` also grew Linux's
aggregate `softirq` line.

**Part 3 — partly done.** Per-task minor-fault counters (field 10 of
`/proc/[pid]/stat`) and `starttime` (field 22) are real now: the fault
handlers charge the task's `min_flt`, and the task's start time is
stamped with the jiffies at which it was built. `/proc/locks` is
implemented in Linux's format (verified against a Linux guest:
`1: POSIX  ADVISORY  WRITE 28848 08:12:76022103 0 EOF`), and the
sysrq `d` command prints the same rows, as Linux's does.

**Part 3 — partly done (cont).** `/proc/diskstats` is implemented in
Linux's layout (the reference host's line:
`   7       0 loop0 14 0 34 2 0 0 0 0 0 1 2 0 0 0 0 0 0`): the block
layer counts reads, writes, sectors and the time spent per device
(`drivers/blkdev/iostat.rs`, the same place Linux keeps them —
block/genhd.c). Rux merges no requests, has no discard path and no
separate flush path, so those columns report zero.

**Part 3 — partly done (cont).** `/proc/timer_list` is implemented in
Linux's structure (verified against a Linux guest: "Timer List Version:
v0.10", the "now at <ns> nsecs" line, the per-CPU clock section and
the "active timers" entries with their expiry). Rux has no hrtimer
clock bases and no per-timer callback, so the two fields with no
counterpart are filled with what the kernel knows — the timer id
instead of the hrtimer struct address, and the action's kind
(alarm/wake/timerfd/oneshot) instead of a callback function name.

**Part 3 — remaining.**

- `/proc/sched` (the per-task runqueue view). Its layout is a ~50
  field per-task block; the reference host does not expose the file
  (`CONFIG_SCHEDSTATS` off), so there is no format to copy — it is
  left alone rather than invented.
- `/proc/pagetypeinfo` — the exact text differs between kernel
  versions and the reference host does not expose the file
  (`CONFIG_PROC_PAGE_MONITOR` off), so the format is not verified yet
  and the file is not added rather than guessed.
- `pgpgin`/`pgpgout` (block-I/O page counters) — Rux has a block layer
  but no per-device I/O accounting in it yet.
- Per-task major-fault counters (field 12): Rux populates file-backed
  pages from the page cache instead of reading from the block device
  inside the fault handler, so there is no fault-time disk read to
  classify yet and the counter stays 0.

Acceptance: `/proc/stat` totals track `/proc/uptime` within one tick per
CPU; `/proc/vmstat` counters are monotonic and reconcile against the
ledger census; a busy-loop guest shows non-zero `user` and rising
`voluntary_ctxt_switches`.

### Tranche 3 — symbols and symbolized traces (done, default-on)

Goal: a backtrace that names its frames.

**Unblocked (RUX-RISCV-LAYOUT-3): the full table is in the default image.**
The build-time symbol pass (`build/gen-ksyms.py`): `nm` over the linked
kernel ELF keeps the text symbols, `addr2line -C` resolves their DEMANGLED
names (the kernel is built with Rust's v0 mangling, so the raw ELF names
are unreadable), and the result is written as a compact table — one
NUL-separated name blob (`ksyms.bin`, embedded with `include_bytes!`)
plus an index of (name offset, address, size, type). `build/Makefile`
runs the pass after the link and rebuilds; `build.rs` writes a
placeholder so a plain `cargo build` still compiles. At runtime
`dfx/ksyms.rs` binary-searches the index, `/proc/kallsyms` prints
the standard `addr type name` lines, and `dfx/backtrace.rs` prints
`[<addr>] rux::mod::fn+0xoff/0xsize` (the classic frame format, with
`?` for an unresolved address) under the `Call Trace:` header.

The table was originally OFF by default (`KSYMS_ENABLE=1` turned it on,
capped at 1MiB of names): the image's address space is budgeted by
hardcoded constants, and the full table (~2.4MB of names + ~0.7MB of
index at ~22.6k text symbols) did not fit the 1.45MB left under the
14MB boot window — worse, the 1MiB cap covered only the first ~1.7MB of
`.text`, so sched/trap/syscall hot-path frames symbolized to `?`.

The fix is the boot-path budget change the layout notes prescribe
(RUX-RISCV-LAYOUT-3 in `main.rs` / `memory_layout.rs`): the image
window grew 14MB→18MB (boot.S PMD tables extended to 9 PMDs each, the
linker ASSERT moved with them) and the heap base, RAM reservation and
`KERNEL_SIZE` moved TOGETHER — reservation-end == heap-start, the
monotone topology the vmemmap backing search requires. Measured on the
merged tree: image 12.55MB→16.0MB with the table embedded, 4.1MB slack
under the new window. The generator now defaults ON (opt out with
`KSYMS_ENABLE=0`) with a 4MiB name budget; the linker ASSERT is the
hard backstop that turns the next outgrowth into a build error.

Guest-verified (`test/kallsyms-smoke.sh`, minimal rootfs): boot reaches
the shell with the heap at the new base (`heap region 128MB @
0x81400000`); `/proc/kallsyms` lists 22,636 lines — the full table;
coverage reaches the end of `.text` (`__etext` resolves at the table's
top); the sched/trap/syscall families resolve by name; sysrq `l`
prints `Call Trace:` with `rux::dfx::backtrace::dump_stack+0x1c/0xdc`.
The two-pass build converges (the table's max text address equals the
final ELF's `_etext`; only `.rodata` grows, `.text` never moves).

### Tranche 4 — tracepoint framework and tracefs

Goal: a general-purpose event facility to replace the ad-hoc rings.

**Done (core, guest-verified).** `kernel/src/trace/` implements the
event table (raw_syscalls:sys_enter, raw_syscalls:sys_exit,
sched:sched_switch), per-CPU ring buffers with one global sequence
counter, and the text renderer byte-shaped after this host's live
tracefs (header block, `%16s-%-7d [%03d] <5 flags> %5lu.%06lu` record
lines). The tracefs side (published under /sys/kernel/debug/tracing)
carries tracing_on, trace, trace_pipe (consuming, no header),
available_tracers/current_tracer (nop), available_events, set_event
(the standard grammar incl. '!' disable and all/none), buffer_size_kb,
the events tree with per-event enable/id/format/filter, per-group and
whole-tree enable with the 1/0/X mixed-state convention, at the
reference file modes. The sched_switch site lives in __schedule just
before context_switch (swapper/N for the idle comm); the syscall pair
brackets syscall_handler_inner with the return read from a0/rax.
Every call site marshals arguments only behind event.armed() (two
relaxed loads), so the disabled path is one branch. Verified from a
guest shell by test/tracefs-smoke.sh.

**Done (event expansion, guest-verified).** Seven more events from the
same shapes, each with the live host's field set and print fmt
verbatim:

- `sched:sched_waking` / `sched:sched_wakeup` — around the enqueue in
  `Task::wake_up` (waking before the attempt, wakeup after it
  succeeded): `comm=%s pid=%d prio=%d target_cpu=%03d`.
- `sched:sched_wakeup_new` — the fork path's first enqueue of the
  built child (`process/fork.rs`).
- `sched:sched_process_fork` — at the child's successful enqueue:
  `comm=%s pid=%d child_comm=%s child_pid=%d`.
- `sched:sched_process_exit` — at do_exit entry, `group_dead` =
  the exit takes the thread group to its last live thread.
- `exceptions:page_fault_user` / `exceptions:page_fault_kernel` — at
  fault entry on both arches (riscv64 `trap.rs`, x86_64 `trap.rs`):
  `address=%ps ip=%ps error_code=0x%lx`. The `%ps` rendering is
  symbolic through the kernel symbol table (Tranche 3's table — the
  two tranches compose), falling back to raw hex exactly as `%ps`
  does when no symbol covers the address. On x86_64 error_code is the
  hardware #PF error word; on riscv64 it is the access-type bits
  (read 0x1 / write 0x2 / exec 0x4) — the arch's fault kind, not the
  x86 error-word layout.

All sites marshal behind `armed()`; every record line renders in the
reference geometry (verified in-guest: `sched_wakeup: comm=sh pid=1
prio=120 target_cpu=000`, `sched_process_fork: comm=sh pid=1
child_comm=sh child_pid=357`, `sched_process_exit: comm=ls pid=356
prio=120 group_dead=true`, `page_fault_user: address=0x3fffffe744
ip=0x28170 error_code=0x2`).

**Done (trace_marker + per_cpu views).** `trace_marker` — the
write-only 0220 file the reference carries: a write records the text
(80-byte cap) whenever `tracing_on` is set, with no event switch
(markers are not enable-gated), rendered as a
`tracing_mark_write: <text>` line in the reference's geometry. Reading
it is refused by the file mode (the reference answers EINVAL to a read
of its write-only file — the same refusal at a different errno).
`per_cpu/cpuN/{trace,trace_pipe,stats,buffer_size_kb}` — per-CPU
slices of the same ring: the trace view carries the header with that
CPU's entries/written counts and that CPU's records only; trace_pipe
consumes one CPU's ring; stats reports the reference's eight fields
(entries, overrun = pushes beyond live capacity, commit overrun (0),
bytes, oldest-event/now timestamps, dropped (0), read events = pipe
consumption). The reference's snapshot and raw views are omitted, not
faked: they need a second buffer and a binary record format this ring
does not have. `trace_marker_raw` likewise omitted.

**Done (tracepoint! macro + the block-IO and binder families).** The
`tracepoints!` table (`kernel/src/trace/mod.rs`) declares every event
once — names, id, the verbatim print fmt and field list, the typed
call signature and the renderer — and the macro derives the static
table row, the typed `trace_<name>` record function (the `armed()`
check inside) and the `format`-file renderer from that one
declaration (divergence register D-4). All ten batch-1 events were
converted with ids unchanged, and the two new families landed through
it:

- `block:block_rq_issue` / `block:block_rq_complete` — the reference
  tracefs field sets and print fmts captured verbatim from this host.
  The sites sit at the two submit chokepoints `/proc/diskstats`
  already counts (the synchronous `request_fn` path and async reads)
  and the completion walkers; `rwbs` carries the direction only
  (R/W/F), `cmd` renders empty, `ioprio` reports the none class.
- `binder:binder_transaction` / `_received` / `_alloc_buf` /
  `_buffer_release` — the reference binder trace definitions verbatim
  (this host's tracefs has no binder group — not compiled in here);
  at the BinderTxn construction, the BR read-out, the buffer carve and
  the free chokepoint.

Verified in-guest (`test/trace-smoke2.sh`, 12/12): the 16-event
table, the six new format files, recorded lines in the reference
geometry for both families, and the group 1/0/X mixed states. One
smoke fix along the way: the block-lines grep moved before the binder
phase — the closed-loop binder probe wraps the 1024-entry per-CPU
ring past the block records, and the first run's greps read an
already-overwritten ring while the events had recorded fine.

**Soak acceptance (done).** `test/trace-soak.sh`: 600 s with every
event armed, tracing on, and a rolling fork/exec/read churn. Zero
panics; cpu0's ring wrapped 1.56 M times at capacity (entries 1024,
overrun 1,559,409, entries-written 1,560,433) with readers
interleaved; the trace, per-CPU and pipe views all rendered coherent
lines afterwards, and the pipe consumption accounting matched (cpu1:
read events 1024, then empty).

### Tranche 5 — crash policy and persistent evidence

**Part 1 — done.** The `panic=` kernel parameter (Linux's
Documentation/admin-guide/kernel-parameters.txt): `panic=N` reboots N
seconds after the panic, `panic=o` / `panic=O` / `panic=immediate`
reboot at once, and with no `panic=` (or `panic=0`) the kernel halts —
Linux's default, and Rux's behaviour until now. The reboot path
deliberately does NOT signal init and does NOT sync the filesystems:
Linux's panic path reboots without touching the disks, because the panic
may well be the storage stack itself, and a sync that hangs in a panic
would defeat the timeout.

**Part 2 — pstore-lite (done).** The last panic's evidence survives the
reboot and reads back through `/sys/fs/pstore`:

- **Reserve.** 128 KiB anchored just below the DTB's physical base
  (QEMU's FDT address is fixed per machine config — 2 MiB below RAM end
  on `virt` — so the region is at the same address every boot AND clear
  of the next boot's FDT rewrite; a naive top-of-RAM carve lands exactly
  on the DTB at the standard `-m` sizes). `memblock_reserve` before
  zone seeding keeps the buddy out.
- **Capture.** The panic handler arms a staging sink first — every
  console byte the report prints (banner, CSRs, registers, the
  symbolized stack) is staged via `Uart::putc` (one relaxed load per
  byte on the disarmed path). At the end of the report the record is
  written: panic identity, the staged report, then the newest printk
  records. The write is allocation-free end to end (the panic may be
  the heap's own failure) and the printk tail is copied under
  `try_lock` only — a stopped CPU may hold the ring, and the panic path
  must still reach the `panic=` timeout (a marker line replaces the
  tail in that case). The magic word is written last: a crash inside
  the capture leaves no record the next boot would trust halfway.
- **Re-expose.** After the linear map is up, a valid magic means the
  last boot panicked; the content stays, the magic is cleared (a clean
  next boot must not re-expose a stale record), and sysfs publishes
  `/sys/fs/pstore/console-ramoops-0` — mode 0444, `pstore_mkfile`'s
  shape, one record deep. No record, no file (the reference directory
  is empty after a clean boot).

Guest-verified (`test/pstore-smoke.sh`, one QEMU, two boots): sysrq
`c` panics boot 1, `panic=3` reboots, boot 2 logs
`pstore: previous boot's panic record found (14741 bytes)`, the file
reads back with the panic identity, the `Kernel panic - not syncing`
banner, the `Call Trace:` header and the console tail (boot 1's
`boot: static regions` line).

Two fixes this depended on: the cgroup boot mount had fabricated a
bare `fs` dentry under the sysfs mount that shadowed the sysfs `fs`
kobject — every `/sys/fs/*` lookup answered ENOENT (readdir still
listed the children: readdir reads the inode, lookup the dentry); the
debugfs flow's pre-lookup discipline now applies to the cgroup path
too. And a guest reset does not clear RAM — the premise the whole
scheme rests on, verified by the reboot cycle itself.

**Remaining (Part 2) — all done, batch 4 (2026-10-26).**

- **Non-fatal Oops path (done).** `dfx/oops.rs`: the kernel fault sites
  that used to take the whole machine down (kernel page fault with no
  exception-table fixup, kernel-mode illegal instruction, unhandled
  kernel exceptions — both arches) now run the reference's contract.
  The report's head line and die banner are Linux 5.15's wording
  verbatim (`Unable to handle kernel paging request at virtual
  address …` / `Oops - illegal instruction` + `Oops [#N]` on riscv64,
  `BUG: unable to handle kernel …` + `Oops: 0000 [#N] PREEMPT SMP` on
  x86_64), the register block is the faulting trap frame in the arch
  `__show_regs` row layout (not a `save_regs` artifact — the SR-PROBE
  lesson), and `Call Trace:` comes from the Tranche-3 symbolized
  walker. Policy = `oops_end`/`make_task_dead`: interrupt context or
  `panic_on_oops` → real panic (pstore + `panic=` apply); idle/early
  boot → panic; otherwise the faulting task dies with SIGSEGV and the
  machine keeps running — the trade the reference makes (whatever the
  task held stays held; the limp-wedge is more diagnosable than the
  halt). `oops=panic` (boot) and `/proc/sys/kernel/panic_on_oops`
  (runtime) restore the loud policy. The recoverable oops is captured
  into the pstore region as the reference's dmesg flavor —
  `/sys/fs/pstore/dmesg-ramoops-0` next boot (the panic record keeps
  `console-ramoops-0`; last writer owns the one region). Taint D and
  the `[#N]` counter land before the report's own `Tainted:` line.
  The old KERNPANIC raw SBI dump and re-entrancy latches stay in front
  of the oops on every site; `/sys/kernel/debug/dfx/provoke-oops`
  (LKDTM's discipline) faults on purpose so the path is exercisable
  from a shell.
- **Persistent log (done).** `/var/log/kmsg` is written again — by a
  klogd kernel thread (khungtaskd's wake pattern), not by the printk
  path. The old per-record synchronous ext4 write (the disabled era's
  filesystem corruption and an unmeasurable hot path) is gone: the
  printk write path now costs nothing extra, the thread walks the ring
  every 30 s with a private seq cursor (the syslog(2) cursor is
  untouched) and appends the delta, newest 512 records per flush, into
  a 256 KiB rotating file (offset wraps, inode size pins the
  high-water mark). `init()` flushes once immediately, so the boot log
  is archived as soon as ext4 is up — early-boot coverage pstore
  cannot give (pstore arms at the panic only, and dies with the power
  like all RAM). The panic/oops paths never write disk: the crash may
  be the storage stack itself, and a sync waiting on a dead queue
  would defeat the `panic=` timeout (the reference kernel's rule).
- **x86_64 pstore anchor (done — the design first).** No firmware
  block sits at a fixed low address there: QEMU rebuilds and relocates
  the ACPI tables near the top of low RAM every boot, so the riscv64
  DTB trick has no twin. The anchor that IS deterministic is the
  kernel image itself: QEMU loads the ELF/bzImage at the link-time
  physical address (0x200000) and the image size is a build constant,
  so `kernel_image_phys_end()` — the same function the mapping treats
  as authoritative — names the same address every boot of the same
  build. The carve sits at [image end, +128 KiB): inside the dead
  window the 30 MB KERNEL_RESERVE already keeps from the buddy (so
  the explicit `memblock_reserve` is belt-and-braces for the day the
  image window grows past it), below every early-boot writer that
  matters (the boot page tables come from a static BSS pool inside the
  image; fixmap-stage tables allocate from memblock AFTER the reserve
  runs and honor it; the boot stack is in the image), and far from the
  ACPI zone. One hazard: an `-initrd` boot — QEMU's linuxboot ROM
  places the initrd at an address that moves with its size, and the
  zero page names the range before any reserve runs, so the carve is
  skipped when the initrd overlaps it (the same no-initrd caveat the
  riscv64 side documents).

Acceptance: `panic=5` reboots the guest within five seconds of a
panic; the next boot shows the previous panic's registers.### Tranche 6 — correctness and lock instrumentation

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

**Perf-support slice (done, batch 4 2026-10-26).** The mandate's
"perf 支持": the smallest useful per-task counter surface the perf
lane's pbench can consume in-guest. The choice:

- **What**: `/proc/[pid]/schedstat` — Linux's CONFIG_SCHEDSTATS
  per-task triple (on-cpu ns, runqueue-wait ns, timeslices), the file
  that turns a bench's wall time into on-cpu vs waiting per task
  (P2 ctxsw, P4 fork/exec). pbench reads it with a plain file read.
- **What not**: `perf_event_open` stays ENOSYS this batch — the
  attr/ring/ioctl ABI is a tranche of its own, and nothing pbench runs
  today needs it. The matrix line stays PART.
- **The gate**: `/proc/sys/kernel/sched_schedstats` (0/1, default 0 —
  the reference's own knob, static-key-backed there since 4.10).
  Rux's static-key equivalent is one relaxed atomic load at each
  schedule edge (the tracepoint discipline); when armed, ONE
  `sched_clock()` read per switch serves both edges — the put (prev's
  on-cpu delta, its exec stamp set at pick) and the pick (next's
  timeslice++, wait window closed — the window opened at
  `enqueue_task_locked`, under the same GRQ lock that guards the pick
  side). A same-task repick (the CPU-bound tick case) is NOT a new
  timeslice — Linux counts switches-to. Counter cost when armed is
  measured by the pbench P2 A-B (off vs on); the disabled cost is the
  one load, covered by the quiet-boot census rows.
- **File shape**: one line, three space-separated decimal numbers,
  `"%llu %llu %lu\n"` — proc.rst's documented triple. The file renders
  (zeros) with the gate off — the honest value of never counted.

The rest of Tranche 7 (the full microbench comparison report) belongs
to the perf lane; the DFX lane's remaining stake is whatever counter
surface that report finds missing.

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

## 8. procfile! adoption (format-drift class, added leg-34 2026-10-25)

The five byte-format defects that survived review (column order,
trailing spaces, wrong-unit timestamps, prefix pollution, shifted
columns) were all hand-written `format!` drift. The fix is now in the
tree: `kernel/src/fs/procfs/procfile.rs` provides the `procfile!`
macro — one field declaration list derives the renderer, the ordered
`*_FIELDS` const, and a mechanical shape test; the converted file also
keeps an exact-bytes snapshot test (the author's conscious update point).
First conversion: `/proc/vmstat` (byte-identical to the hand-written
version, snapshot-pinned).

ADOPTION RULE for this lane's files (this is the coordination point
between the perf lane, which owns the macro, and the DFX lane, which
owns most /proc additions):

- Every NEW /proc key-value file uses `procfile!` — no new hand-written
  `"<key> {}\n"` format strings.
- Existing files convert opportunistically when touched. The `pad`
  form exists since leg-35 (meminfo shape: `%-16s` key+colon, `%8lu`
  right-justified value, ` <unit>`; a `!`-suffixed key renders
  unitless — Linux's HugePages_* rows carry no `kB`). Second
  conversion: `/proc/meminfo` (declaration order preserved; the
  conversion intentionally REPLACED the old lines' four different key
  paddings and left-flowed values with the reference kernel's exact
  column layout — verified against the live reference /proc/meminfo;
  order+shape pinned by `meminfo_file_tests`, no exact-bytes snapshot
  because the values are live). The stat-family files convert the
  same way when next touched. The `cols` form exists since leg-36
  (stat-family ROW shape: label prefix, then one space-separated u64
  per declared column — the derived row function's ARGUMENT LIST is
  the column list, so a call site cannot swap two counters without
  the names disagreeing at the call). Third conversion: `/proc/stat`
  (aggregate `"cpu "` + per-cpu rows via `stat_cpu_row`; the kv tail
  through `render_kv`; the dynamic-arity `intr`/`softirq` sum lines
  stay explicit — no fixed field list to derive; whole-file LINE
  ORDER pinned by `stat_file_tests::line_order_check`). Fourth:
  `/proc/schedstat` (`schedstat_cpu_row`; Linux's literal middle zero
  kept as the named `alias0` column). Remaining hand-written stat-
  family candidates: `/proc/interrupts`, `/proc/softirqs`,
  `/proc/diskstats` (dynamic-arity row tails, like `intr`).
- The declaration ORDER is the file's byte order; treat reordering as
  a Linux-observable change and snapshot-test it.
