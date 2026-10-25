# Design Divergence Register

Internal design points where Rux deliberately keeps a different design from
the mainstream kernel approach, because our design carries an advantage —
gated by three conditions that every entry must hold:

1. **External compatibility intact** (syscall/ABI/ELF/filesystem interfaces
   behave to spec).
2. **LTP green** on the affected families.
3. **No upper-layer software breakage** (unmodified distro/OS images boot
   and run their full service set).

Entry template: what the divergence is · the advantage · the compatibility
evidence · the known cost · the guard that watches it (perf-gate domain or
probe). New entries land when the design reaches `main`; entries are never
deleted — superseded designs get a closing note.

---

## D-1: Global run queue (single GRQ)

**Design.** One global run queue serves all CPUs. The mainstream approach is
per-CPU run-queues plus a cross-CPU load balancer; Rux has no per-CPU
run-queue and no load balancer — the pick path is a single queue with
per-priority-class ordering (RT/CFS bands), with the idle path per-CPU.

**Advantage.**
- Single source of truth for scheduling order: no migration decisions, no
  balancer heuristics to tune, no fairness bugs from imperfect balancing.
- O(1)-class pick with a tiny state space — the entire wake/pick/strand
  forensics stack (W3 census, TERM-DESCHED, WAKE-REFUSED probes) reasons
  over one structure, which is how the scheduler bug families of the OH
  campaign were actually found and fixed.
- Measured hold cost is flat and small: GRQ average hold 5.1–6.1 µs across
  loads (volume-coupled, not contention-bound at current scale).

**Compatibility evidence.** LTP sched-family tests green; 4-CPU SMP boots
Ubuntu 22.04 to a desktop and OpenHarmony to its full ~80-service set with
launcher execution in-sandbox.

**Known cost.** A single queue is a contention ceiling at high CPU counts
and cache-locality loss versus per-CPU queues. Acceptable at MAX_CPUS ≤ 4
(current config); revisit if core counts grow.

**Guard.** Perf-gate scheduling domain: GRQ average hold band (5.1–6.1 µs
±20%), census in band (TERM 8±2 / REFUSED 32±4), pbench P2 context-switch
median ±20% — any change touching `sched/` reports these.

---

## D-2: Packed kernel-stack slab (x86_64)

**Design.** Kernel stacks are carved three-per-256KB order-6 block (80KB
slots) by a dedicated slab with a side-table slot map and empty-block
reclaim, instead of the mainstream buddy allocator's
one-power-of-two-block-per-request (a bare 80/96KB request burns a whole
128KB order-5 block). riscv64 and unmeasured arches compile the slab out
entirely and keep plain 128KB allocation — packing buys nothing at
power-of-two sizes, and the 80KB margin is an x86_64-measured figure
(the ungated first attempt corrupted adjacent heap data on the deeper-
nesting riscv64 test kernel; the arch gate is itself regression-tested).

**Advantage.** Per-stack footprint 85.3KB vs 128KB (~-115MB at the
launcher-floor population where stacks were 42% of the 904MB floor);
99.9% slot packing live (sc237: kb=981/ks=2941).

**Compatibility evidence.** No external interface touched; riscv64 suite
1096/1096 (the ungated variant read 1094/1096 — the gate caught it);
x86_64 full-image boot to deepest-ever population (3445 live tasks) with
zero canary hits; census exact hold across the gate boots.

**Known cost.** A leaf side-table lock on the alloc fast path; 6.7%
per-block tail waste; the ~15KB depth margin is arch-specific and thin.

**Guard.** R18-2 canary at every slot bottom; KSTACK-HW depth probe
(dfx=schb,ksprobe); SC-ANAT kb/ks packing columns; perf-gate allocator
domain (Tier 1 census/ticks, Tier 2 fork rows).

---

## D-4: Derived proc-file formatters (procfile! macro)

**Design.** /proc text formats derive from one field-declaration list
(kv/pad/cols forms) instead of hand-written format strings; the
declaration mints the renderer, an ordered FIELDS/COLUMNS const, and a
shape test; the cols form derives the row function's argument list from
the column list.

**Advantage.** Kills the format-drift class (five byte-format defects
survived review into a merged tree once); format and data become one
artifact; call sites cannot swap columns.

**Compatibility evidence.** vmstat byte-identical snapshot; meminfo
column-exact vs the live host reference; stat/schedstat line order
unit-pinned; full suite green; three full pbench boots DONE rc=0.

**Known cost.** Opportunistic conversion — ~20 hand-written files
remain; dynamic-arity lines (interrupts/intr/softirqs) stay explicit
by design.

**Guard.** procfile-shape unit-test group + per-file order/snapshot
pins; the DFX plan's adoption rule requires the macro for new files.

---

## D-5: tracepoint! declaration macro (T4 event table)

**Design.** One tracepoints! table row per event derives the static
row, the typed record function (armed() check inside), and the
format-file renderer; ids stable across conversions.

**Advantage.** Event declaration is a single site; the format file and
the record code cannot drift apart; block-IO/binder/sched/page-fault
families all declared uniformly.

**Compatibility evidence.** trace-smoke2 12/12 after conversion (16
events, all format files, recorded lines); 600s armed soak prior,
re-verified post-macro.

**Known cost.** Macro indirection for new event authors.

**Guard.** trace-smoke2 + the shape tests; hot-path cost is the
armed() single-branch check measured by syscost per the merge gate.

---

<!-- Pending entries (register when they land on main):
     - packed kernel-stack slab (order-6 blocks carved into 80KB slots,
       vs plain buddy order-power allocation) — on the launcher lane branch,
       arch-gated, register at merge with its sc228-class evidence. -->

## D-3: Build-time kernel symbol table in .rodata (two-pass convergence)

**Design.** The runtime symbol table (the backing store for
`/proc/kallsyms` and symbolized stack traces) is generated at BUILD
time — `nm` + `addr2line -C` over the linked ELF, demangled, compacted
into a NUL-separated name blob plus a (offset, address, size, type)
index — and embedded in the kernel image's `.rodata`, instead of being
carried in a separate file loaded at boot or read back from the ELF
headers at runtime. Only TEXT symbols are recorded, which is what
makes the two-pass build converge: the table lives in `.rodata`, and
`.rodata` sits after `.text` in the link order, so embedding the table
never moves `.text` — the addresses the table records for pass 1 stay
valid in pass 2 (verified: the table's max text address equals the
final ELF's `_etext`).

**Advantage.**
- Zero boot-time setup and no runtime dependency on the boot loader or
  a debug-info channel: symbolization works from the first panic
  printer frame, on every image, by default.
- One compact representation (2.4MB of names + 0.7MB of index at ~22.6k
  symbols) shared by every consumer (kallsyms, backtraces, `%ps`
  rendering in trace events, sysrq dumps).
- The generated artifacts are gitignored; `build.rs` writes a
  placeholder so a plain `cargo build` (no symbol pass) still compiles.

**Cost.** The image grows ~3.1MB; the boot window, heap base and RAM
reservation moved 14MB->18MB together for it (RUX-RISCV-LAYOUT-3;
x86_64 needs no move — its mapping derives from `__kernel_end` at
runtime and the heap sits at 1GB phys).

**Compatibility evidence.** `/proc/kallsyms` prints the standard
`addr type name` lines (22,636 lines, coverage through `_etext`);
`Call Trace:` frames render `name+0xoff/0xsize` with `?` for
non-text addresses; unit suite 1096/1096 with the table embedded;
minimal-rootfs boot verified with the moved layout.

**Guard.** The riscv64 linker.ld `ASSERT(_end <= window)` — the next
outgrowth fails the build, not the boot; `test/kallsyms-smoke.sh`
checks the count, the end-of-text coverage and a symbolized backtrace.

---

## D-4: Single-declaration tracepoint table (tracepoints! macro)

**Design.** Every trace event is declared exactly once, as one row in
the `tracepoints!` table at `kernel/src/trace/mod.rs`: group, name,
id, the verbatim `print fmt` and field list, the typed call signature
and the renderer. A declarative macro derives the three artifacts a
hand-written site used to carry — the static `EVENTS` table row, the
typed `trace_<name>(...)` record function (the `armed()` check inside,
so call sites marshal nothing when the event is off), and the
`<name>_fmt` renderer behind the `format` file. The mainstream approach
spreads this over a `TRACE_EVENT()` class macro plus generated
codepaths plus a codegen-time format file; here one Rust macro is the
single source the compiler checks, in-tree.

**Advantage.**
- The `format` file, the record call and the table row cannot drift
  apart: they are three expansions of one declaration (the derive
  pattern of `docs/architecture/rust-design-headroom.md` §4). A new
  event is one table row plus its call sites — the six-event batch-2
  expansion (block-IO + binder families) needed no new table
  scaffolding.
- Typed call sites: field types are checked at the call, so a wrong
  argument order is a compile error, not a malformed trace line.
- The disabled path stays one branch: `armed()` is two relaxed loads
  inlined into the generated call function, arguments marshal only
  behind it.

**Cost.** Ten u64 argument slots per record cap the per-event payload
(a comm costs two slots) — events wider than ten slots need a wider
slot array, not a new mechanism. The renderer is a closure per event,
so render bugs surface at the trace reader, not the record site (the
smoke greps catch them).

**Compatibility evidence.** The tracefs surface stays byte-shaped
after the live host: `available_events`, the per-event `format` files
(verbatim field sets and print fmts, captured from
`/sys/kernel/tracing/events/` on this machine) and the record lines
verified in-guest for all 16 events (`test/tracefs-smoke.sh`,
`test/trace-smoke2.sh` — block_rq_issue/complete and the four binder
events, 12/12 checks); 600 s armed soak with zero panics and coherent
views after 1.56 M ring overwrites (`test/trace-soak.sh`).

**Guard.** Perf-gate Tier-1/Tier-2 (the record sites sit on the
syscall dispatch, scheduler and block-IO paths): per-op syscall
averages and pbench rows must hold band with every event disabled —
the disabled cost is the thing the gate watches.
