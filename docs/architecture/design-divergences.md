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

**Known cost.** Opportunistic migration — the only struct-literal
tables left are drm x2 (GNOME-lane files, convert when that lane
rests); everything else builds (D-4 procfile precedent). The refactor
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
pattern, the data-guard proof chain).

**Known cost.** Opportunistic migration (D-4/D-6 doctrine): unkeyed
locks keep compiling; only pairs named in lock-ordering.md adopt keys,
and adoption must not widen a subsystem's public API. The
guard-less `try_lock_claim`/`unlock` pair exists for the GRQ's
hand-rolled guard protocol and must not grow new users.

**Guard.** `lockkey` unit-test group; INV-LOCK-5/6 remain watchdog-
and SPIN-verified in the meantime; the sched/ side of any adoption
rides the scheduling-domain perf gate (GRQ hold band, census, P2).

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

**Compatibility evidence.** First conversion: the file-locks subsystem
(`fs/locks.rs` — flock, POSIX/OFD record locks, leases): the four
public functions return `Result<(), Errno>`; the six call sites
(sys_flock in syscall/io.rs, F_SETLK/F_SETLEASE in vfs.rs, three
break_lease propagation sites) each map once with `as_neg_i32()`.
Errno values byte-identical (EAGAIN/EINTR/EINVAL/EDEADLK; the enum
gained the missing `Deadlock = 35` variant). Suite green; the LTP
fcntl/flock families are the behavioral guard.

**Known cost.** Opportunistic adoption: the i32 errno convention
dominates the tree and keeps compiling; subsystems convert when
touched. Mixed APIs coexist exactly as D-4/D-6 conversions do.

**Guard.** LTP fcntl/flock families; the errno_test unit group pins
the enum values.
