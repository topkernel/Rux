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

**Status (user ruling, 2026-10-28): the GRQ stays.** Per-CPU queueing was
re-evaluated and rejected as over-engineering at 4 cores with low
contention; the performance lane's follow-up is GRQ slimming only, not a
queue-model change. Two anchors for whoever revisits D-1 later:

- Churn control is pinned by configuration, not left to accident:
  `cfs_min_granularity_ns = 700_000` with the wakeup granularity at the
  same value, at a 100 Hz tick — a task that is prequeued cannot lose the
  CPU before ~0.7 ms of service, and the 10 ms tick bounds the re-queue
  rate, so wake storms do not churn the queue.
- The "small hold cost" claim above is decomposed by measurement (wake
  hold = runqueue accounting / dequeue-enqueue / pick / stats segments,
  from the wakewatch/WW census instruments); future D-1 work should
  compare segment-wise against that decomposition rather than only the
  5.1–6.1 µs aggregate.

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
(kv/pad/cols/kvtab forms) instead of hand-written format strings; the
declaration mints the renderer, an ordered FIELDS/COLUMNS const, and a
shape test; the cols form derives the row function's argument list from
the column list. The kvtab form (`Key:\t<value>\n`, one Display value
per row, preformatted composites for multi-part rows) carries the
/proc/[pid]/status family, with STATUS_FIELDS pinning the key sequence
against a live render; kvtab_pad (`key<tabs>: value\n`, padding in
the key, empty value drops the space) carries /proc/cpuinfo.

**Advantage.** Kills the format-drift class (five byte-format defects
survived review into a merged tree once); format and data become one
artifact; call sites cannot swap columns.

**Compatibility evidence.** vmstat byte-identical snapshot; meminfo
column-exact vs the live host reference; stat/schedstat line order
unit-pinned; full suite green; three full pbench boots DONE rc=0.

**Known cost.** Opportunistic conversion — ~18 hand-written files
remain (the derived set grew in the refactor lane's slices: diskstats
via cols with the merged/discard/flush zeros named as columns and an
exact-bytes snapshot pin; /proc/[pid]/status via the kvtab form with
its key sequence pinned against a live render; /proc/cpuinfo via the
kvtab_pad form — per-arch declared rows whose keys carry the tab
padding, per-block order pinned; the x86 list preserves the
pre-conversion duplicate processor row until the coordinator decides
that fix); dynamic-arity lines (interrupts/intr/softirqs) stay
explicit by design.

**Guard.** procfile-shape unit-test group + per-file order/snapshot
pins; the DFX plan's adoption rule requires the macro for new files.

---

## D-5: tracepoint! declaration macro (T4 event table)  [superseded by D-15, kept for history]

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

## D-6: Capability-checked file-ops declaration (§1 builder + one-place missing-op dispatch)

**Design.** A file type declares its operations as capabilities
through a const `FileOps::builder()` — each builder method installs
exactly one op slot and refuses duplicates (a duplicate is a const
panic, i.e. a compile error in every `static` initializer), so the
capability matrix of a type is a stated fact instead of an
inferred-from-`None` wall. The behavior of a MISSING capability is
decided in one place, not per call site: `File::missing_op_errno`
(Read/Write → EINVAL, LSeek → ESPIPE, ReadAt → EINVAL for op-less
types vs ESPIPE for streams) and `File::poll_dispatch` (per-type poll
callback or the always-ready mask) own the whole mapping; the
mainstream approach (function-pointer struct, null checks, errno
convention at each dispatch site) re-derives it everywhere and gets it
wrong under load — the paid incident: read() on a valid epoll fd
returned EBADF because the read slot was NULL, and an OH event loop
spun 155,991 iterations on the wrong errno before the semantics were
fixed by hand, per-site.

**Advantage.**
- The errno/fallback mapping exists once; a new fd type cannot pick a
  wrong convention by copying the wrong neighbor call site.
- The capability matrix is pinned per type by the `fileops-caps`
  unit-test group — adding a slot (e.g. giving epoll a read op) fails
  a shape test before it can drift into the tree.
- Duplicate capability installs are compile errors; absent slots are
  declared absences.
- Hot path unchanged: same fn-pointer indirect calls, same slots; the
  dispatch helpers inline to the same loads/branches.

**Compatibility evidence.** Unit suite 1103/1103 with the new group
(run/pa58-maketest.log); errno matrix pinned to the ABI-documented
values (EINVAL/ESPIPE classes above); poll/select/epoll call sites in
misc.rs all route through the one dispatcher with
behavior-preserving fallbacks (both old fallback shapes reduce to the
same event bits). First conversions: the §1 incident family
(epoll/eventfd/timerfd/signalfd/pipe/inotify).

**Known cost.** Migration COMPLETE: the last two struct-literal
tables (drm x2 — DRM_OPS/PRIME_OPS, GNOME-lane signed off) converted
in the refactor lane's final slice; every FileOps table in the tree is
now a capability declaration (D-4 procfile precedent for the
opportunistic doctrine that got here). The refactor
lane converted: slice 3, the pseudo-filesystem cluster (devfs x7 incl.
the null/zero/full/random char devices, pty x2, procfs, sysfs,
debugfs, selinuxfs, rootfs, memfd, fifo, char_dev uart, cgroup, vfs
mem-file — 19 tables); slice 4, the streams/devices tail (kmsg x2,
watchdog, evdev, fbdev, nsfile — 6); slice 5, the socket family and
ipc/device stragglers (inet/unix/raw/netlink sockets, access_token,
tokenid, ashmem, io_uring, binder — 9), all pinned in `fileops-caps`.
The builder does not yet catch a FIRST
install of a capability a type should not have — that guard is the
shape test, per converted type.

**Guard.** `fileops-caps` unit-test group (capability matrix + errno
matrix); Tier-1 census and the p5 family rows of the perf gate cover
the poll/epoll hot paths (any change touching this dispatch reports
them).

---

## D-7: Typed per-CPU storage (§3 CpuLocal — the irq discipline moves from review to type)

**Design.** A per-CPU bank `CpuLocal<T, N>` holds one cache-line-
aligned row per cpu and is reachable ONLY through `with(f)`, which
disables interrupts, reads the cpu id, and hands `f` a `CpuLocalRef`
guard; the guard is `!Send` (cannot migrate to another cpu), cannot
outlive the closure, and its `Drop` restores the saved IRQ state — so
the interrupt-disabled window a per-CPU access depends on is exactly
the guard's lifetime, a type-level fact. The mainstream approach this
replaces is a `static mut` array indexed by a raw cpu id with a
review-enforced "callers must have IF=0" comment; the paid incident
family is the small_alloc magazine divergences — r8final (t=515 s,
two-CPU row entry via a cpu_id()/ti_cpu divergence, kernel panic at
the row) and the 2026-10-26 transient two-CPU entry fixed on main
(7a6e1bf4) — both were that convention failing silently. Two
tripwires survive as DATA, not review notes: an out-of-range cpu id
takes the `None` arm (callers fall back to their locked bodies — the
r8final survive-and-taint stance), and a same-slot re-entry is refused
(counted + tainted; the outer window stays flag-correct).

**Advantage.**
- The aliasing-by-construction hazard (two raw `&mut` into one row)
  is gone; the re-entry tripwire turns a would-be r8final into a
  counted, tainted refusal.
- Rows are cache-line padded — the old flat `[T; MAX_CPUS]` let
  adjacent CPUs' magazine rows share lines on every fast-path op.
- Every `save_and_disable_irq`/`restore_irq` pair at a converted site
  is the guard's acquire/drop — no manual flag juggling to audit.

**Compatibility evidence.** First conversion: the small_alloc
magazine bank (the incident site itself) — alloc fast path, locked
refill, free fast path incl. the full-row snapshot/flush/post-push
sequence; all r8final len/MAG_DEPTH guards kept verbatim (they defend
against row CORRUPTION, which the type cannot prevent). Unit suite
green with the new `percpu-shape` group (round-trip, window = irq-off
window, nested different slots, re-entry refused+counted+tainted,
zero-bank None arm, guard cpu identity).

**Known cost.** `T: Copy` for the const row initializer (per-row
constructors later); the in_use check is one plain load+branch on a
line the cpu already owns (measured by the perf gate when a converted
site is hot — small_alloc rides the Tier-1 domain rows). The GRQ
typed lock key, the other §3 item, is the follow-on slice.

**Guard.** `percpu-shape` unit-test group; the small_alloc REST-share
domain row and Tier-1 census rows of the perf gate cover the
converted allocator hot paths; and — binding since the incident
below — a release-strong pbench boot before any re-land/extension of
this type (debug-only verification cannot see the codegen family).

**Incident and fix (2026-10-26/27, run/pa68-d7-rootcause.txt).** The
first landing placed the bank in a non-mut `static` whose type had no
interior mutability (`[Row<T>; N]` + `unsafe impl Sync` — the Sync
impl does NOT confer mutability), so rustc emitted the LLVM global as
`constant` (.rodata). The kernel image maps P|RW, which hid it from
hardware: debug builds emitted the stores and booted; under opt>=1
LLVM deleted every store into the bank and hoisted the loads — the
magazine refill loop never terminated and every release-profile boot
died at the first c64 miss (ALLOCTHROW t=3 s, 15.9 M objects carved
across 2 lock holds, 99% of the heap). Reverted as 95b0015f; re-landed
with `rows: UnsafeCell<[Row<T>; N]>` — interior mutability is the
piece that makes the mutation legitimate to the compiler instead of
invisible to it. Post-fix artifacts verified: MAGAZINES in .bss,
refill-loop stores present in the opt>=1 disassembly, release-strong
boot clean.

---

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

## D-15: Single-declaration tracepoint table (tracepoints! macro)  [renamed from a duplicated D-4]

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

---

## D-8: Write-through jbd2 commit (no replay window, no revoke cancels)

**Design.** The jbd2 commit persists a transaction by syncing its
registered buffers THROUGH to their final on-disk locations (the
write-through loop), then advances the log tail/head past the
transaction's journal-area records in the same commit. A completed
commit therefore leaves nothing replayable: recovery starts at s_start,
which already points past the records the commit just wrote. The
journal-area copies remain on the slow path for format compatibility
only. Consequence: jbd2 revoke records need none of Linux's
revoke-cancel machinery (JBD2_FLAG_REVOKE_CANCEL). A revoke stamped
with its issuing transaction's tid can never suppress a descriptor of a
strictly later transaction (the filter is `revoke.tid >= desc_tid`
with strictly increasing tids), and same-transaction suppression is
benign because the suppressed replay is redundant — the content is
already at its final location via the write-through pass. Revokes are
per-transaction sets (lazy, bounded by a revoke high-water), not a
journal-global force.

**Advantage.**
- The per-op synchronous journal commit that Linux's revoke semantics
  would otherwise force on every unlink/rmdir (descriptor + copies +
  revoke + commit + superblock, 8-12 round trips) is gone: delete-class
  operations batch exactly like writes (p6_mkdrmdir 52-58 ms strong
  median class → single-digit ms; see
  docs/development/f4-ext4-route-map.md §6).
- One persistence model to reason about (write-through + dirty-buffer
  drain points) instead of two (write-back journal replay for metadata,
  write-through for the rest) — the crash-consistency stance is uniform:
  recovery is e2fsck, as on a write-back mount of the reference kernel.

**Cost.** The journal-area records written by the slow path are dead
weight (born dead per the tail advance); a future real
checkpoint-replay journal would have to drop the tail advance or write
replayable records. Same-transaction free-then-realloc of a block skips
its replay — harmless only while replay stays redundant, i.e. while the
write-through pass runs on every commit path.

**Compatibility evidence.** The on-disk jbd2 format is unchanged
(descriptor/revoke/commit blocks, header tids, recovery scan); journal
images mount and recover unchanged. LTP mutation families (creat,
unlink, rmdir, rename, fsync classes) green; pbench fs rows in band
(the fs domain gate); fsw jcommit_slow drops to batch count under
delete load.

**Guard.** fs-domain perf-gate rows (p6_mkdrmdir, p6_unlink,
p6_write_fsync, p6_append) plus the fsw jcommit counters; the
release-strong boot requirement for any change touching this path.

---

## D-9: Kernel-side LD_* environment scrub on secure exec

**Design.** When an exec transitions credentials (setuid/setgid bits
taken, or euid!=uid/egid!=gid — the AT_SECURE condition), the KERNEL
removes every `LD_*` environment variable from the envp it installs on
the new image, before ld.so ever runs. The mainstream approach leaves
the environment untouched and sets AT_SECURE=1; the userspace dynamic
loader scrubs its own environ copy.

**Advantage.**
- Defense in depth independent of libc behavior: glibc and musl both
  scrub when AT_SECURE, but static binaries with custom libc stacks,
  future loaders, or partially-linked images get the guarantee from the
  kernel unconditionally.
- The scrub also covers loader-tunable variables a given libc's secure
  list may not know yet.

**Compatibility evidence.** The trigger condition is exactly the
reference kernel's secureexec rule (review P15-era fix: capabilities
alone no longer mark every root exec secure). Ubuntu 22.04 and OH images
boot with their full service sets (every setuid helper — mount, su,
appspawn — passes through this path). LTP exec family green on the rows
that exercise envp handling (execve01/02/03/05/06 pass; the loader
behaves identically because it re-scrubs what is already absent).

**Known cost.** One visible divergence: `/proc/<pid>/environ` of a
secure-exec'd process does NOT list LD_* strings, where the reference
kernel's environ (read from the initial stack region) still shows them —
ld.so there only repoints the environ array, the stack strings remain.
Cosmetic-to-benign (no known consumer relies on seeing LD_* in a setuid
process's environ), recorded so the difference is a registered design,
not an accident.

**Guard.** The exec ladder (kernel/src/syscall/process.rs do_execve) is
the single scrub site; resolve_exec_file (session 1) keeps the
AT_SECURE inputs on the one-walk resolution path, and the shebang-parse/
resolve_exec_file unit rows plus the LTP execve-family spot run watch
the surrounding semantics.
## D-11: Typed lock keys — the lock hierarchy as trait impls

**Design.** Lock-ordering classes carry zero-sized key types
(`GrqKey`, `FutexBucketKey`, `NetQueueKey`, `NetStatsKey`); a
PERMITTED nesting is an explicit `impl CanNestInside<Outer> for
Inner`. A held keyed guard mints a zero-sized `NestProof` (guards are
the only minters — no public constructor), and acquiring a keyed lock
while another is held consumes that proof (`lock_nested(&proof)`,
`lock_irqsave_nested(&proof)`), which only compiles when the impl
exists. The proof's lifetime ties it to the guard, so it cannot
outlive the held section. The mainstream approach this replaces is a documented
lock-ordering file plus review plus a post-hoc deadlock watchdog; the
paid incident class is any INV-LOCK-* reverse-order introduction —
review-enforced discipline that silently fails under refactor pressure
(the futex-wake path nests GRQ inside the bucket lock on every wake;
the reverse order is an instant machine-wide deadlock).

**Advantage.**
- The nesting table is not documentation — it is the impl list; a
  reverse or same-level acquisition of migrated pairs does not compile.
- The key name reaches the deadlock watchdog output (class-level
  attribution even before a pair migrates).
- Zero runtime cost: the key is `PhantomData`, the acquire paths
  inline to the raw protocol's instructions.

**Compatibility evidence.** First adoption is the GRQ's own lock —
internal type swap (`RawSpinlock` -> `KeyedRawSpinlock<GrqKey>`), the
GrqGuard API, the irqsave/preempt protocol and every call site
unchanged. SECOND SLICE (coordinator-approved): the fence is
LOAD-BEARING — the futex `HASH_HEADS` buckets are
`KeyedSpinlock<FutexBucketKey, Option<usize>>` (same lock_irqsave
discipline, Deref/DerefMut call sites unchanged), and futex_wake's
R12-2 wake-under-bucket call is `Task::wake_up_in_futex_bucket(task,
&head.proof())` -> `wake_up_enqueue_nested` ->
`GlobalRunQueue::lock_irqsave_nested`, a chain whose every hop
demands the proof; the bucket->GRQ nesting is now a compile fact
(the reverse direction remains watchdog-enforced). The requeue path's
same-class ordered double-bucket acquire (lo/hi) stays on the plain
API by design. Unit suite green with the `lockkey` group extended
(guard hold/drop, INV-LOCK-5/6 forward nesting, the GRQ claim/unlock
pattern, the data-guard proof chain). THIRD SLICE: the futex
WAITER_POOL slot locks are `KeyedSpinlock<WaiterSlotKey, Option<Waiter>>`
(`impl CanNestInside<FutexBucketKey> for WaiterSlotKey`); the
wake-chain walk and the diagnostic scan acquire proof-checked via
`KeyedSpinlock::lock_nested`; the top-level allocator/free paths and
the requeue relink window's same-class double-slot holds stay on the
plain API by design.

**Known cost.** Opportunistic migration (D-4/D-6 doctrine): unkeyed
locks keep compiling; only pairs named in lock-ordering.md adopt keys,
and adoption must not widen a subsystem's public API. The
guard-less `try_lock_claim`/`unlock` pair exists for the GRQ's
hand-rolled guard protocol and must not grow new users.

**Guard.** `lockkey` unit-test group; INV-LOCK-5/6 remain watchdog-
and SPIN-verified in the meantime; the sched/ side of any adoption
rides the scheduling-domain perf gate (GRQ hold band, census, P2).

**Incident, conviction and root cause (2026-10-29, the 2b092c49
withdrawal).** The first GRQ adoption (`KeyedRawSpinlock<GrqKey>`
field swap, 13-boot paired-bisection convicted, withdrawn in 623c4736)
failed by an API-SHAPE bug, not by the keyed design and not by a GRQ
protocol race: `GlobalRunQueue::lock_irqsave`'s pre-existing
`self.lock.lock();` statement was written for `RawSpinlock::lock()`,
which returns `()`. Under the wrapper, `KeyedRawSpinlock::lock()`
returns a guard — the statement DISCARDED it, and the guard's Drop
released the lock at the end of the statement. Every "GRQ-locked"
section in the convicted kernel ran unexcluded: concurrent CFS
timeline mutation (x86 6/6 navigate.rs unwrap panics) and concurrent
picks of one task (riscv64 R74-DOUBLE-SEED double-run, torn context).
The GRQ pick/ownership protocol itself is exonerated — the R74 shape
was the expected downstream of a lock that never held. Fixes landed
with this entry: both keyed guard types are `#[must_use]` (a
discarded guard is a compile warning naming the conviction), and the
claim surface (`lock_claim`/`try_lock_claim`/`unlock`) exists so a
protocol-wrapping subsystem never touches the guard-returning `lock`.
Any re-adoption of the GRQ swap goes through `lock_claim` and a fresh
adjudication.

---

## D-10: Result-typed internal error APIs (§5 pilot)

**Design.** Internal kernel functions return `Result<T, Errno>` (the
typed enum, not raw negated `i32`s); the negative-errno ABI mapping
happens exactly once per call chain, at the syscall/VFS boundary via
`Errno::as_neg_i32()`. The mainstream approach threads `-errno` ints
through every layer, where sign bugs and errno-mangling recur (the
paid incidents: the u32-microsecond wrap that killed an SA host; the
negated-nice encoding bug).

**Advantage.** An `Err(-EAGAIN)` cannot be accidentally re-negated,
compared against a byte count, or returned where a positive errno was
meant — the error type is an enum, and `Result` is `#[must_use]`, so
an ignored failure is a build warning, not a silent drop.

**Compatibility evidence.** Conversions so far: (1) the file-locks
subsystem (`fs/locks.rs` — flock, POSIX/OFD record locks, leases):
the four public functions return `Result<(), Errno>`; the six call
sites (sys_flock in syscall/io.rs, F_SETLK/F_SETLEASE in vfs.rs,
three break_lease propagation sites) each map once with
`as_neg_i32()`; the enum gained the missing `Deadlock = 35` variant.
(2) the xattr subsystem (`fs/xattr.rs` — check_name/may_set/set/get/
list/remove, 20 error sites, the densest raw-errno file in fs/): all
typed, the internal ?-propagation type-uniform, the do_* wrappers in
syscall/file.rs mapping once per arm; the enum gained
`ResultTooLarge = 34`, `NoData = 61`, `OperationNotSupported = 95`
(the file's two local errno-constant duplicates are deleted). Errno
values byte-identical throughout; suite green; the LTP fcntl/flock
and setxattr/getxattr/listxattr/removexattr families are the
behavioral guards. (3) the VFS inode-operations table (`fs/inode.rs`
- the lookup/create/symlink/mkdir/mknod/iget slots, their op_*
wrappers, and the 26 implementer functions across tmpfs/ext4/procfs/
devfs/rootfs/sysfs/selinuxfs/debugfs/cgroup): the whole create-class
path-resolution API is typed. The deep internal layers not yet
converted (ext4 namei/inode, the sched cgroup helpers) bridge through
`Errno::from_neg_i32` / `From<i32>` - a documented, visible marker to
retire as those layers convert.

**Known cost.** Opportunistic adoption: the i32 errno convention
dominates the tree and keeps compiling; subsystems convert when
touched. Mixed APIs coexist exactly as D-4/D-6 conversions do.

**Guard.** LTP fcntl/flock families; the errno_test unit group pins
the enum values.

---

## D-12: Window batch-fill on the ext4 read miss path

**Design.** On a page-cache miss, the whole miss window is filled in one
round trip instead of per-page: `ext4_file_read_cached_dst` computes the
window (demand range; plus the sequential read-ahead window when the
access looks sequential, or a 16-page fault-around cluster for the
page-aligned single-page signature that mmap demand faults produce),
`fill_page_cache_batch` resolves each uncached page's physical block,
COALESCES physically-contiguous runs into single multi-block device
requests (`bio::bread_async_multi`, up to 64 blocks / 128 KiB per
request), submits the batch back-to-back, kicks the device once, and
drains everything with one sleep/wake cycle (`drain_batch`). A
coherence guard splits any run at the first block-cached block so the
direct DMA can never bypass a live dirty buffer-cache entry (the Xorg
xkbcomp lesson). The mainstream kernel serves filemap reads/faults
page-by-page through the per-page bio machinery with separate
ondemand-readahead heuristics; its fault-around maps the cluster but
each page still rides the individual fault path.

**Advantage.** Under virtio-blk (TCG in particular), per-request fixed
cost — chain setup, header/resp allocation, device kick, IRQ — dwarfs
per-byte DMA cost. Before this design every page miss paid one
synchronous submit/sleep/wake (~1.8 ms under TCG); the batch turns a
whole window into one wait, which is what makes the desktop images
(Linux distro boots, OpenHarmony's ~100 MB of ArkUI libs faulting at
load) bootable at all. The read-ahead and fault-around windows piggyback
on the same batch at no extra round trips.

**Compatibility evidence.** External read semantics unchanged
(read/pread/mmap EOF, short-read, EFAULT and hole-zeroing contracts;
the Err-vs-hole split of 070770d6 made the error arm match
ext4_file_read_iter's). LTP read-family green; the perf lane's pcache-B
stage 1 wraps this exact path (P3d concurrent-fault pair green,
B/A = 0.84 on the paired board).

**Known cost.** The whole miss path (mapping resolution, DMA, page-cache
inserts) runs under EXT4_BIG_LOCK — a deliberate correctness-first
choice (the ftest01/ftest05 write/read coherence conviction); concurrent
misses on different files serialize at the fill. Run coalescing is
capped at 64 blocks/request and 512 blocks/batch; a 10s-device-stall
discipline leaks (never frees) buffers the device may still DMA into.

**Guard.** Perf-gate filesystem hot paths domain: pbench P3 fault rows
and buffer-cache hit rate >= baseline; the exthunt read-side probes
(rd_serve/rd_batch/rd_hit/READHOLE) adjudicate serve-truth when a
read-corruption family appears.

---

## D-13: Batched metadata publication + discard-on-free (ext4 write path)

**Design.** Two write-path stances, landed with the perf-fs sprint
(2f219478, 1debc2b1, 39975867). (1) Publication records — the
crash-ordering layer over deferred metadata writes — batch per
transaction window: `bio::publication_batch_drain` fires at every
`ext4_journal_stop` bracket close only when the pending queue crossed
PUB_BATCH=64, instead of each entry mutation paying a scoped ordered
drain of its own records at op return. (2) Freed blocks are FORGOTTEN:
`free_block_run` drops each freed block's dirty unreferenced
buffer-cache entry without writing it and removes its jbd2 registration
(`jbd2_journal_forget`) so no later commit re-reads and rewrites the
dead content. Alongside, `getblk_overwrite` serves full-block metadata
rewrites without the disk read the old `bread` paid (the reference
never reads a block it is about to fully rewrite).

**Advantage.** p6_mkdrmdir med 29796 us -> 3742 us (publication
batching) -> 2107 us (discard-on-free), p99 54 ms -> 16.4 ms, against
the reference kernel's 153 us -> within ~14x on the shared
syscall/lookup floor (p6_stat 15 us vs reference 0.23 us is that floor,
not the fs). Dead directory blocks from create/delete churn are never
synchronously written; the revoke-threshold slow commits stop
round-tripping stale bytes through the journal area.

**Compatibility evidence.** POSIX never promised per-op directory
durability — that belongs to fsync(dirfd); Rux's durability points
(fsync/sync(2)/eviction/commit) and the H1/H2 out-of-band constraint
guards are unchanged. Crash ordering rides the bitmap/GD/SB deferred
writes plus revoke records. The forget stance mirrors the reference's
own EXT4_FREE_BLOCKS_FORGET semantics; clean/pinned freed buffers are
left in place. Fresh directory blocks are fully initialized in-memory
(zeroed Vec + entry + csum tail) and written via the full-block path,
so the skipped read can never leak prior-life bytes. `make test` and
the pbench P6 battery green on both arms (journal / no-journal).

**Known cost.** Up to PUB_BATCH records' worth of dirty metadata in
flight between durability points — an out-of-band writer hitting a
constrained block pays a constraint flush bounded by that many
records' blocks. A pinned-or-clean freed buffer survives the forget
and is written later (dead data to a free block: harmless to the
bitmap truth, not optimal). discard-on-free does not TRIM the device.

**Guard.** Perf-gate filesystem hot paths domain: pbench fs rows
(mkdir+rmdir/unlink medians) within band; fsw counters pb2/pm (batch
drains, queue max) and FSW3 db= (drops) watch the queue length and
drop volume; the buffer-cache hit-rate floor catches over-dropping.

---

## D-14: Measured-wait wakeup-preemption gate (CFS)

**Design.** On a wake that found no idle CPU, the busy-CPU
need_resched poke runs only when the wakee's measured wait —
sched_clock minus its last pick stamp — is at least one
SCHED_LATENCY_NS window (6 ms); a wakee that last ran within the
window is left to the slice/handoff machinery (977adb51). The
mainstream kernel gates wakeup preemption on a vruntime-credit
comparison (check_preempt_curr / wakeup granularity). Rux's vruntime
floor-on-enqueue and place-on-wake adjustments would make that raw
comparison pass on every wake of a ping-pong receiver, re-creating a
preempt dance per record.

**Advantage.** Kills the per-record preempt dance on same-CPU
sender/receiver pairs: a 74-byte unix-stream record cost ~275 us of
wake/preempt/switch dance against the reference's ~2.5 us; the idle
steering + gate brought the p5 socket family 10-18x. Senders run
unmolested at trap exit while receivers drain in parallel on idle
CPUs.

**Compatibility evidence.** Scheduling policy only — no ABI surface.
A wakee older than one latency window keeps immediate wakeup
preemption (the starvation/latency case the mechanism exists for);
sub-window wakees are picked at the incumbent's next scheduling point
(slice end, cond_resched, lock slowpath), which is the same batching
the reference applies to wakeup-granularity-immune pairs. LTP
sched-family green; pbench P2/P5 rows in band; census TERM/REFUSED
bands unchanged.

**Known cost.** Wake latency for sub-6 ms ping-pong receivers is
bounded by slice expiry rather than immediate preemption; the exec_start
approximation (last pick stamp) slightly overestimates the true wait
for tasks whose producers hold the CPU across their pick.

**Guard.** Perf-gate scheduling domain: pbench P2 context-switch and
P5 IPC medians within band; census (TERM 8±2 / REFUSED 32±4); the
ipcdisc battery's drain-shape discriminators.
---

## D-16: Deferred-free timer ring (wall-clock stand-in for deferred reclamation)

**Design.** Freed `MmStruct`/`Task` blocks enter a global 16384-entry ring
drained after a 1500 ms wall-clock deadline (`mm/deferred_free.rs`), sized
by a length-class match, instead of grace-period-based reclamation
(call_rcu) for these objects. The QSBR RCU (`sync/rcu.rs`) exists and is
used where readers are enumerable; the mm/task blocks take the ring
because their borrowers (procfs walkers) are unaccounted `&MmStruct`
borrows that a quiescent-state discipline cannot see cheaply.

**Advantage.** Reclamation latency is bounded and independent of
scheduler activity; no reader-side instrumentation on the hot walk paths.

**Compatibility evidence.** The fake-OOM/stale-tree hunt families closed
under it; full suite green; OH boots unaffected.

**Known cost.** 1500 ms retention per freed block (memory headroom, not
correctness); the ring is a single global lock — fine at current object
rates. Revisit: unaccounted borrows → RcuHead on the structures would let
call_rcu serve them and delete the ring.

**Guard.** meminfo pcp/deferred rows; OOM-forensics census on hunts.

## D-17: Kernel-half device alias arena (no identity MMIO window)

**Design.** Device MMIO is reached exclusively through kernel-half alias
windows — two static windows plus a dynamic ioremap arena
(`mmu_init::ioremap_bar_kernel`) — and the bootstrap identity map is
UNLINKED from the kernel CR3 after the boot-stack switch. The mainstream
approach keeps an identity window and ioremaps on demand into the vmalloc
space; here there is no low identity window at all after boot, and BARs
(including firmware-placed low MMIO like the RTC page at 0x101000) map
into the arena.

**Advantage.** One MMIO discipline under every CR3 (trap-context MMIO
under a user page table cannot fault); the uaccess contract has no
identity hole (copy_from_user at low user addresses faults instead of
reading physical memory — the prlimit64 EFAULT gap this closed).

**Compatibility evidence.** x86_64 suite 1194/1194 with the window gone
(including the previously-red EFAULT row); RTC/virtio-mmio/virtio-BAR
consumers converted and green; OH boots green.

**Known cost.** Arena is bounded (registry slots); a BAR outside every
window needs an explicit ioremap call at probe time.

**Guard.** PF-NOCTX beacon silent across the suite; SMAP-VIOL beacon
silent.

## D-19: Typed timer wheel — timer classes enforced by the type system

**Design.** The internal timer subsystem enforces timer classes by
type (kernel/src/timer/, landed through 060ad4c2/09664c48/0b396384):
clock-typed deadlines and now-readings (`Deadline<C>`/`Time<C>` — a
jiffy-domain value and an hres-domain value cannot be compared), split
one-shot/periodic arming constructors (`arm_oneshot_jiffy/hres` takes
no interval parameter; `arm_periodic_*` requires `NonZeroU64` and has
no `PeriodicAction::Wake`), and caller-side scoped wake guards
(`WakeArmed`/`WakeSpent` in oneshot.rs — the guard owns the timer id,
is not Copy/Clone, `cancel` and `rearm_if_spent` consume it, Drop
cancels on early return). Stored-id families (itimer/POSIX/timerfd)
deliberately keep the raw-id facade API. External ABI unchanged.

**Advantage.** Three historical timer bug families — the hres/jiffy
mixed-clock compare (lost-wakeup early fire), one-shot
leak/mis-re-arm, and double-del/stale-id-del — are now compile errors.
Measured, same-rig A/B (s5-boot, OH image, KVM q35 smp2/4G, syscost=1,
780 s; A = old flat-table kernel, B = typed wheel): timer arm lock
hold 924→697 ns, pending 1288→868 ns, del 1291→862 ns per op at
matched volumes (n within 0.1%); nanosleep avg 772.6→662.7 ms;
pbench p2_ctxsw 12.198 µs vs the 15.400 µs anchor (improvement side,
in gate). Six rustc type-catch classes recorded on the real tree
(lane artifact arch-s5/run/arch-s5-s2/type-catches.txt: E0308
clock-domain mismatch, E0599 no periodic-Wake, E0308 NonZeroU64,
E0061 one-shot+interval has no signature, E0308 spent-vs-armed guard,
E0382 use-after-move on double cancel).

**Compatibility evidence.** make test 1306/1306; riscv64 scripted
regression boot green (0 panics); OH boot 0 panics/OOM; census in
class; clock_gettime same class (3 µs) with an 18x deeper boot;
caller migrations landed (futex backoff/wait_timeout, nanosleep) with
~27 further raw-facade files sharing the scoped shape.

**Known cost.** Slot 56→64 B (pool 66 KB .bss); the guard's Drop adds
one no-op find on fired exits (same cost as the old explicit del);
stored-id families keep the untyped facade by design.

**Guard.** Tier-1 quiet-boot census + clock_gettime/nanosleep rows;
pbench P2 (scheduling domain); the timer-wheel shape-test group in the
unit suite.

---

## D-18: SMAP enablement with EFLAGS.AC pushfq sequences

**Design.** x86_64 enables CR4.SMAP with the usercopy primitives
bracketing their copies with pushfq/orq/popfq AC set/clear sequences
instead of the STAC/CLAC encodings. The semantics are identical (AC gates
supervisor access to user pages); the pushfq form is chosen because the
STAC/CLAC encodings take #UD under QEMU TCG regardless of CR4.SMAP
(bisection-proven 2026-10-31), which would make the KVM-class rig
unbootable under the emulator fallback.

**Advantage.** Identical default-deny semantics on hardware AND under
TCG; no acceleration-mode-dependent kernel image.

**Compatibility evidence.** x86_64 suite green with SMAP on and the
identity window dropped; strong pbench board in band (p4 best-ever
456µs); zero SMAP-VIOL beacons — the syscall layer is fully on uaccess.

**Known cost.** pushfq/popfq is a few cycles heavier per bracket than
STAC/CLAC on hardware; measured invisible under the benchmark rigs.

**Guard.** SMAP-VIOL beacon on every boot; the uafault guest probe (11
syscalls × bad pointer → EFAULT) in the packed probe set.

---

## D-20: Writer-side dirty writeback watermark on a free-fraction mark (no global dirty accounting)

**Design.** Buffered-write throttling (kernel/src/fs/ext4/file.rs +
kernel/src/mm/vmscan.rs, falloc-wedge lane 2026-11-16): once the
Normal zone's free count falls below HALF its managed pages
(`zone_below_dirty_writeback_mark`), the buffered writer itself flushes
its own inode's dirty range and then runs a system-wide recovery pass
(`PageCache::writeback_recover`: free already-clean pages first, then
collect dirty inodes via a full-table rotating walk and flush each
whole inode through the run-batched engine, evicting the flushed
inode's pages directly up to mark + 1/16 managed). The reference kernel
expresses the same discipline (writeback far below the memory wall,
writers throttled in their own context) through
dirty_background_threshold + balance_dirty_pages over a global dirty
page count per backing device; Rux keeps no per-BDI dirty accounting,
so the mark is a zone free-fraction instead. The discipline, not the
constant, is the contract; the exact constant is recorded here as a
divergence.

**Advantage.** No new global lock or per-BDI state on the write path —
the mark reads two zone counters (read-only atomics, any context), and
the exact global DIRTY_PAGES counter that exists alongside is census
only (adjusted under the per-inode entry lock that already serializes
every transition — exactness by construction, zero added locking).
The watermark eliminated the delayed-allocation dirty-balloon wedge
class: a sustained buffered writer (mkfs/fill through a loop device)
used to grow the dirty set to RAM scale (243k pages), dirty pages
skipped eviction, the zone drained to zero, and reclaim's own
nested, io-lock re-entering, allocation-starved flush was the last
producer standing (the fallocate05/06 runner wedge: reclaim io_lock
re-entry + fdreap RCU stall). With the watermark, the fallocate05
ENOSPC fill completes and LTP fallocate05/06 pass (3/3 fresh boots
each, RC=0, 16/24 TPASS, zero TFAIL — run/shell-f56-fin{1,2,3}.log).

**Compatibility evidence.** LTP fallocate05/06 3/3 + 3/3 PASS on the
landed tree; mm-family storm (111 runs, 37 tests x3) zero forensics
events of every family (run/res-storm1.txt); triple-arch builds rc=0;
riscv64 make test 1328/1328 (run/maketest-final.log). WB-WATERMARK
tuning prints show the recovery un-tripping the mark (free 137k vs
mark 121k after flush+shrink rounds, boot f56-fin1).

**Known cost.** The free-fraction mark is coarser than the reference's
dirty-fraction: it also trips when CLEAN page cache legitimately owns
half the zone (the recovery's shrink stage handles exactly that — it
un-trips the mark by evicting clean pages, measured shrunk=45k pages
in one 498-jiffy round). First-40-per-boot WB-WATERMARK prints bound
the log cost.

**Guard.** WB-WATERMARK print rows (free must rise above mark, dirty
must fall); PCACHE-RECLAIM census line (SR_*/EV_*/FL_* faces) on the
OOM-FORENSIC block; pbench fs rows (stat/mkdir+rmdir/unlink/
write+fsync medians) for the buffered-write path; the fallocate05/06
pair as the standing regression probe.

---

## D-21: Default-on bounded poll-before-sleep for synchronous I/O waits

**Design.** `IoCompletion::wait` (kernel/src/fs/io_completion.rs:188-212,
submit-2x lane, d4769538e) polls its `done` flag for a bounded budget —
1536 PAUSE iterations ≈ 14 µs on the 3.7 GHz KVM rig — with IRQs and
preemption armed, BEFORE arming the 10 s deadline timer and scheduling
out; the sleep path runs unchanged for completions slower than the
budget. This is the DEFAULT for every synchronous block-I/O wait in the
kernel. The reference kernel (7.0.0-38-generic, mainline shape) never
polls before sleeping: polling exists only as an opt-in mode
(`blk_mq_poll`/`blk_rq_poll`, block/blk-mq.c, behind REQ_HIPRI/
REQ_POLLED from io_uring HIPRI or aio). The reference's closest
historical relative, the v5.15 hybrid poll
(`blk_mq_poll_hybrid`/`blk_mq_poll_hybrid_sleep`/`blk_mq_poll_nsecs`,
block/blk-mq.c — since removed from mainline), was (a) opt-in per
queue, (b) adaptive — budget = half the per-IO-size-bucket mean
completion time, and (c) an hrtimer sleep, not a busy spin.

**Advantage.** Under KVM the whole sleep machinery (timer arm/disarm,
prepare_to_wait, schedule-out, cross-CPU IRQ wake, schedule-in, wait
teardown) costs more than the device round trip itself (22-28 µs), so
skipping it for in-budget completions is a direct win at depth 1:
same-window kernel-vs-kernel interleaved A/B — 4k randrw direct
14.95k→22.15k IOPS/side (+48%, reference parity on the same rig), 4k
randwrite +25%, 4k randread +16%, 1M seq read +6%
(/home/william/rux-agents/submit-2x/run/SUBMIT-2X.md §3-E, §4). Safe
by construction here because the waiter at depth 1 has nothing else to
run and the budget is bounded; the poll is also self-disarming on any
device whose completions exceed it.

**Compatibility evidence.** No guest-visible ABI change (an internal
wait policy); fio --verify=md5 clean on 4k buffered and 64k direct;
riscv64 make test 1330/1330; triple-arch builds rc=0; Tier-2 pbench fs
rows in band (SUBMIT-2X.md §7). LTP unaffected (no interface change).

**Known cost.** Burns up to the budget of CPU per synchronous op (a
fixed spin where the adaptive v5.15 heuristic would spin less or
sleep); under multi-job depth-1 load the aggregate spin is real (8
jobs × 14 µs windows). The fixed budget sits where the adaptive
mean/2 would land for THIS device (RT 22-28 µs → mean/2 ≈ 11-14 µs);
on a slower device the fixed budget is strictly worse than the
adaptive form. Follow-up hook: a per-completion mean/2 budget is the
reference's improvement, adoptable without changing this entry's
shape.

**Guard.** pbench/fio 4k direct rows (the poll is the dominant term
there); the perf-gate Tier-2 fs rows; syscost census IRQ-share bands
(IF=0 share — the poll runs with IRQs armed, so no new IRQ-off class
may appear).

## D-22: Inline-hardirq virtio-blk completion walker on a single fixed MSI-X vector

**Design.** The virtio-blk MSI-X completion path (dio-conc lane, f051ef80e)
runs the pending-table completion walker INLINE in hardirq context —
IDT vector 0x81 (kernel/src/arch/x86_64/trap.rs:1288-1297) — with a
bounded try-lock that defers to the Block softirq when a task-context
submitter holds the BLK lock (R12-3 discipline). The vector is
programmed once at queue setup, fixed to physical CPU0
(virtio_pci.rs:102-108). The reference driver does the split the other
way: `virtblk_irq` (drivers/block/virtio_blk.c) only marks completion
and calls `blk_mq_complete_request`, which runs the actual completion
in SOFTIRQ context (local BLK_MQ_SOFTIRQ or an IPI to the submitting
CPU), on per-vq MSI-X vectors that typically land on the submitting
CPU.

**Advantage.** Kills two measured per-op latencies on the 4k direct
path: the INTx ISR-poll phase (16.3 µs — every registered virtio
function's ISR cap polled per interrupt) and the softirq hop (17.1 µs)
that the pre-MSI path paid: 4k randread direct 11.8k→34.1k IOPS,
83→27.6 µs/op for this lever alone
(/home/william/rux-agents/dio-conc/run/DIO-CONC.md §3-§5); with
submit-2x's poll (D-21) the 4k rows sit at 0.90-0.97 of the reference.
The inline walker is bounded (try-lock, defer on contention), so no
IF=0 spinning on submitters.

**Compatibility evidence.** No guest-visible ABI change. ~50 clean
boots of the final kernel, no panics/MSI storms (DIO-CONC.md §6);
fio --verify=md5 clean; riscv64 make test 1328/1328; INTx remains the
fallback for caps-less devices and the net device; riscv64 PLIC and
aarch64 GIC paths untouched.

**Known cost.** Hardirq context is more brittle than softirq: any new
lock taken by the walker must be irqsave-or-trylock (the defer class
is the guard). The fixed CPU0 vector means the completing CPU is
usually NOT the waiter's CPU at j8 (mitigated by D-21's polling: the
polling CPU observes `done` via cache coherence). Runtime MSI-X table
steering toward the submitting CPU is a CONCLUSIVE dead end on this
QEMU/KVM (any runtime table write kills completion delivery —
SUBMIT-2X.md §3-C; recorded to prevent blind retries).

**Guard.** perf-gate Tier-3/IRQ domain (IF=0 share, max closed cli
segments, SOFTIRQ dispatch share — the inline walker must not create
a new IRQ-off class); 4k direct fio rows; the assert_no_virtio_lock
debug invariant on completion paths.

## D-23: Byte-snapshot settle guard for shared metadata blocks (jbd2 buffer-ownership, journal-less edition)

**Design.** The wrfsync lane's SpanFlusher (kernel/src/fs/ext4/
file.rs:4390-4450) submits the inode-table block (and any shared
metadata block) in the SAME device window as the data spans — the
jbd2 "prepare everything, one burst, one drain" shape without a
journal. Because a NEIGHBOR inode's write can update the same table
block between snapshot and settle, each in-flight span carries a
byte-snapshot of the block it persisted, and BH_Dirty is cleared only
if the buffer still matches the snapshot; a raced block stays dirty
and is re-synced. The reference solves the same race with journal
buffer ownership instead (fs/jbd2/transaction.c do_get_write_access:
a buffer registered in a committing transaction blocks editors — no
byte comparison exists in the reference). The guard is self-invented.

**Advantage.** Removes the second serialized device round trip per
fsync (~30 µs on the fio rig: the ITB sync was its own drain) while
keeping the wrongly-cleared-BH_Dirty class — the double-allocation/
csum corruption family — impossible by construction: a raced update
leaves the block dirty rather than silently marking it clean.
Measured: fsync4k 4653→20.1-22k IOPS vs reference 15.3-16.2k
(0.30x→1.24x ahead)
(/home/william/rux-agents/wrfsync/run/WRFSYNC.md §3).

**Compatibility evidence.** fio --verify=md5 (buffered randwrite +
fsync) clean; in-guest md5sum of 1G/2G test files match host digests;
riscv64 make test ALL PASS; Tier-2 pbench rows in band or improved
(WRFSYNC.md §5-§6). The fsync durability contract is unchanged (the
settled block's persisted bytes are the ones the fsync owed).

**Known cost.** One block-sized byte compare per raced settle (rare;
the common case is no race) and a per-span snapshot field. The guard
is journal-less-only by construction; on journal mounts the reference
ownership discipline governs and the guard is bypassed.

**Guard.** fio --verify=md5 with fsync; the ext4-corrupt lane's
reference-boots-our-/data zero-errors check; fsw census counters for
new sync-write classes (perf-gate fs domain).

## D-24: Per-inode BTreeMap page cache with single-walk entry() install (xarray analogue)

**Design.** The page cache (kernel/src/fs/page_cache.rs:477) keeps each
inode's pages in a per-inode `Spinlock<BTreeMap<u64, CachedPage>>` —
safe-Rust std BTreeMap, no unsafe, no lock-free radix tricks — with the
batched install (`insert_batch_ext`, :2748) doing ONE map descent per
page (the old shape was contains_key + contains_key + insert = three
descents). Dirty/writeback state lives in a side-table
(DIRTY_DATA_BLOCKS, ext4/file.rs:2175) rather than in-tree tags. The
reference's structure is the xarray (lib/xarray.c) with XA_TAG_* bits
in the per-address_space radix tree, single descent on insert, and
tag-batched iteration (filemap_get_folios_tag, mm/filemap.c:2375).

**Advantage.** Safe Rust end to end (the xarray is a lock-free
structure the reference maintains with careful memory ordering and
must audit for races); per-inode lock granularity without a shared
tree; the bufw lane measured the single-walk install alone at
lock-pass 94k→70k cycles/op on the 1 MiB buffered write
(/home/william/rux-agents/bufw-2x/run/BUFW-2X.md §2-§3, §7: row
0.83→~0.94 of reference, min-lat at parity).

**Compatibility evidence.** No guest-visible ABI change (pure
internal structure). fio --verify=md5 buffered 4k clean including the
unaligned head-partial shapes the lane fixed; seq2g/rand1g guest
digests match host truth; riscv64 make test 1330/1330; guard rows in
band (BUFW-2X.md §6-§8).

**Known cost.** O(log n) with allocation per node growth (xarray is
allocation-amortized via internal entries); global side-table tags
are an extra indirection vs in-tree tag bits; iteration is not
batched the way xarray tag iteration is (the fsync walk compensates
by being O(dirty-notes), D-23/§A3).

**Guard.** pbench p6_read_cached/p6_append rows; the fs-domain
perf-gate rows; fio verify suites; PCACHE-RECLAIM census faces.

## D-25: Adaptive lock-free child memo over the authoritative dentry tree (dcache-lookup analogue)

**Design.** Directory child lookup (kernel/src/fs/dentry.rs:380-389,
529-534, stat-2x lane 78b17d528) keeps the authoritative
`children: Spinlock<BTreeMap<String, Arc<Dentry>>>` and adds
lock-free read tiers in front: an inline 4-slot memo, then a lazily
allocated `BigMemoTable` — a direct-mapped table of seqlock slots,
sized adaptively next_pow2(2 × live children) clamped [128, 2048],
stamped with the existing `children_ver` discipline, retired through
the reader-quiescent drain (DENTRY_RET_MEMO). The reference performs
lock-free lookup directly over its AUTHORITATIVE persistent structure
(the dcache: `__d_lookup_rcu` with `d_seq` seqcount, fs/dcache.c:1148)
— there is no memo because the hash IS the lookup path, and dentries
persist with LRU aging rather than being children-map entries.

**Advantage.** Memory proportional to use: 8 KiB (small dirs, never
allocated below 12 children) to 128 KiB (≥1025-entry dirs) per grown
directory, vs a global hash sized for the whole namespace; no LRU
aging problem (the authoritative map is the truth, the memo is
disposable); adaptive sizing measured NECESSARY (a fixed 128-slot
table vs a 1000-name tree gained nothing — direct-mapped thrash —
while 2048 slots took stat 0.491→0.291 µs; the adaptive form picks
2048 there and 128 small)
(/home/william/rux-agents/stat-2x/run/STAT-2X.md §2-§3: stat
2.23x→1.28x, openclose 2.43x→1.92x, read_cached 2.18x→1.84x of
reference).

**Compatibility evidence.** No guest-visible ABI change. riscv64 make
test 1328/1328; riscv64 regression boot clean; pbench guard rows in
band (STAT-2X.md §3, §5); no new invalidation rules (children_ver
stamping predates the lane).

**Known cost.** Two structures to keep coherent (memo + map) where
the reference has one; a direct-mapped memo can thrash at sizing
misfits (the clamp bounds this; growth reallocs under the children
lock); lookup still falls to the locked BTreeMap on memo miss (the
reference's rcu-walk never takes the dcache lock on hit).

**Guard.** pbench p6_stat/p6_openclose rows; the fs-domain perf-gate
band; LTP rename/unlink families (memo invalidation correctness).
