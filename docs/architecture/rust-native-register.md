# The Rust-Native Register — designs Rux ships that a translation could not

Purpose (user directive 2026-11-03): every architectural advantage —
performance, safety, correctness, observability — is RECORDED HERE with
evidence, so a reader sees original engineering, not a C-to-Rust
transcription. The companion register for interface-level divergences is
docs/architecture/design-divergences.md (D-numbers); this file carries
the narrative, the evidence, and the "why a translation would not have
produced this" argument. RULE: an entry without measured evidence is a
placeholder, not an entry.

## How to read the register (the thesis, first)

Rux is a reference-compatible kernel: the syscall ABI, ELF/exec
contracts, filesystem formats and boot behaviors match the reference
kernel closely enough that unmodified distro and OpenHarmony images run
their full service sets, and LTP runs green on the affected families.
The internals are NOT a transcription. They are Rust-native originals —
designs whose correctness or speed comes from the type system, the
ownership model, or a deliberate internal architecture the reference
kernel does not have — and every one of them carries a measured number
from this repository's own instruments.

Three reading rules:

1. **Every entry passed the three-gate rule** (external POSIX/ABI
   compat intact + LTP passes the affected families + no upper-layer
   software breaks). Divergence is earned, not asserted.
2. **"Translation would not have produced this"** means: the design
   depends on Rust's type system, ownership model, or
   safe-construction guarantees such that a line-by-line port of the
   reference kernel structurally cannot arrive at it — the bug class
   the design eliminates is a COMPILE ERROR here, or the data structure
   the optimization needs is expressible only under ownership.
3. **Evidence conventions.** Decisive numbers are copied INTO this file
   so it is self-contained; every entry names its source: a `main`-tree
   document, a commit id on main, or a lane artifact with its full path
   (lane worktrees under /home/william/rux-agents/<lane>/). Where a fix
   ADOPTED reference semantics (the honest opposite of divergence), it
   is filed under section C, not claimed as an original.

Companion documents: design-divergences.md (the D-number register),
os-kernel-survey.md + industry-design-survey.md (what the industry
ships and Rux's adoption verdicts), security-design.md (the layered
defense), docs/development/perf-gate-baseline.md (the merge-gate
numbers this register cites).

---

## A. Type-system-enforced designs (bug classes that are compile errors here)

- **D-19 Typed timer wheel** (kernel/src/timer/, commits 060ad4c2 /
  0b396384): one-shot vs periodic arming split by constructor
  (`arm_oneshot_*` takes no interval; `arm_periodic_*` requires
  `NonZeroU64` and has no `Wake` action), deadlines and now-reads
  typed by clock domain (`Time<JiffyClock>` vs `Time<HresClock>` cannot
  compare), and scoped one-shot guards (`WakeArmed`/`WakeSpent`, guard
  owns the id, `cancel`/`rearm_if_spent` consume it, Drop cancels on
  early return). Three historical timer bug families — hres/jiffy
  mixed-clock early fire, periodic mis-re-arm, double-del/stale-id-del
  — are now unrepresentable. Evidence: six rustc type-catch classes
  planted and caught on the real tree (E0308 clock-domain mismatch,
  E0599 no periodic-Wake variant, E0308 NonZeroU64, E0061 one-shot+
  interval has no signature, E0308 spent-vs-armed guard, E0382
  use-after-move on double cancel; lane record
  arch-s5/run/arch-s5-s2/type-catches.txt). Same-rig A/B (s5-boot, OH
  image, KVM, 780 s): timer lock holds per op arm 924→697 ns, pending
  1288→868 ns, del 1291→862 ns at matched volumes (n within 0.1%);
  nanosleep 772.6→662.7 ms; p2_ctxsw 12.198 µs vs the 15.400 µs
  anchor; suite 1306/1306. Source: arch-s5 lane TASK.md session-2
  record (commit 14013b5a).
- **S-1 Model-checked concurrency primitives** (kernel/verify/,
  commits 11c73f42 / 1d697ec2 / 3b7b6f68): the R84/R85 SleepMutex
  handoff state machine extracted into a mirror crate with zero kernel
  restructuring, then loom-interleaved (8 scenarios, 59.5 s, preemption
  bound 3) and Kani-proved (INV-M1..M5, 2.2 s). It caught 3 planted
  bugs including a pre-fix wedge shape, and found a real pid-0
  recursion bug (commit 011a71c5) in its first 90 s of use. The
  exhaustive result that matters: two-task pure-fair contention is
  strand-free WITHOUT the anti-starve timer; with 3+ waiters, strand
  interleavings exist that only the R85 timer bounds — a measured
  justification for a design feature, not a heuristic.
- **D-11 Typed lock keys** (sync/, cross-ref D-11 in the divergence
  register): lock-ordering classes are zero-sized key types; a
  permitted nesting is an `impl CanNestInside<Outer> for Inner`; a
  held guard mints a `NestProof` that `lock_nested(&proof)` consumes —
  reverse-order acquisition of migrated pairs does not compile, at zero
  runtime cost (PhantomData keys). The conviction that hardened it:
  the first GRQ adoption was withdrawn after a 13-boot paired
  bisection (2b092c49/623c4736) because a discarded guard silently
  released the lock — both keyed guard types are now `#[must_use]`, so
  that exact API-shape bug is a compile warning naming the conviction.
  Evidence: `lockkey` unit group green; futex bucket→GRQ and
  waiter-slot→bucket nestings are compile facts on main.
- **D-7 Typed per-CPU storage** (CpuLocal<T,N>, cross-ref D-7): rows
  reachable ONLY through `with(f)`, whose guard disables interrupts,
  is `!Send`, cannot outlive the closure, and restores the saved IRQ
  state on Drop — the interrupt-disabled window a per-CPU access
  depends on is exactly the guard's lifetime, a type-level fact. This
  replaces the raw `static mut` array + review-enforced "IF=0" comment
  whose silent failure was the r8final two-CPU-entry panic family
  (t=515 s). The landing incident is itself register-grade evidence of
  what the compiler checks that review cannot: the first version sat in
  a non-mut static, LLVM placed the bank in .rodata, debug builds
  booted and every opt≥1 boot died at the first cache miss — caught by
  disassembly verification, fixed with `UnsafeCell` (root-cause record:
  perf-audit/run/pa68-d7-rootcause.txt).
- **WalkClaim reader banks** (fs/dentry.rs, commit 3f377d13): the path
  walk's per-component reads (child map, inode Arc) run lock-free
  behind a typed claim — a SeqCst increment of THIS CPU's reader-bank
  counter whose `&WalkClaim` is the only currency the lock-free read
  methods accept; writers publish to the not-current bank and retire
  the old value through the deferred-free ring once both banks drain
  (the FdTable lever-B protocol, generalized). The two per-component
  spinlock pairs it removed measured ~39 ns (children) + ~20 ns
  (inode) per component (fsw pathcost probe). Spot A/B: stat
  0.643→0.423 µs, open 1.128→0.889 µs. History kept honest: the first
  landing was reverted (724867a0) when it shifted heap timing onto a
  pre-existing buffer-cache zeroing defect; re-landed after that family
  was fixed, with stale-memo routing hardened (ea26d923).

## B. Designs where the safe-by-construction shape IS the optimization

- **D-1 Global run queue** (sched/, cross-ref D-1): one run queue
  serves all CPUs — no per-CPU run queues, no load balancer, no
  migration decisions; the pick path is a single queue with
  per-priority-class ordering and per-CPU idle. The translation trap
  it refuses: porting the reference's per-CPU-queue + balancer design
  would carry its balancer heuristics and fairness surface wholesale.
  The measured dividend is tractability — the entire wake/pick/strand
  forensics stack (W3 census, TERM-DESCHED, WAKE-REFUSED, WAKE-
  STRANDED probes; see F) reasons over ONE structure, which is how the
  OH campaign's scheduler bug families were actually found, decoded,
  and heal-verified. Banked numbers (perf-gate-baseline.md): census
  TERM-DESCHED 8 / WAKE-REFUSED 32, EXACT and unchanged across every
  accepted quiet boot since 2026-10-25 (band 8±2 / 32±4); GRQ average
  hold 2786 ns at main@afc41b8b (n=6.3M; the 4080-7320 ns gate band),
  flat against a 126.0M-syscall boot — volume-coupled, not
  contention-bound, at the current ≤4-CPU config (the registered
  known cost and revisit condition).
- **Leaf-table direct unmap walk** (mm/x86_64, commits 1d9c8b75 /
  ee42c599): ownership made the per-page walker allocation-free; the
  walk resolves the upper chain once per 2 MiB block and iterates leaf
  PTEs directly. The measured wall it removed was self-inflicted — a
  cond_resched-per-chunk inside munmap (a translation artifact the
  reference never had: 132 µs of yield per call, n=213k/60 s, max
  1.64 s) — so the honest framing is: the redesign both removed our
  own artifact and adopted the reference walk shape, and the
  allocation-free leaf walk is the piece a port could not carry over.
  Numbers: mmap avg 72 µs (max 749 ms) → 27-32 µs (max 2.1-3 ms);
  munmap body 177-185 → 52 µs; fixmun 210 → 34 µs; >16 MiB mmap
  median 5.6 ms → 415-427 µs; the vm-op family wall under OH load
  ~15 s/CPU-min → ~5 s/CPU-min. Source: code-review-perf lane TASK.md
  (lb325 vs lb340/341 syscost, mmprof attribution).
- **Negative-lookup memo with inode-generation stamping** (dcache,
  commits 7c8cd5d3 / d2b2672a): invalidation keyed on the parent
  inode's dir_mut_gen — alias-safe across bind mounts by construction
  (a stale negative entry cannot survive a directory mutation because
  the key it is stamped with changes). Oracle evidence: the negstorm
  probe (create/link/rename-in/mkdir/symlink churn over shared
  parents, 8- and 16-worker shapes, 180 s each) exercised the memo
  with 475k negative hits and 963k stale normalizations across both
  boots with ZERO stale-negative deliveries, ghost entries, or
  verification failures. Source: stress-port/run/stress-runs/findings.md
  (round-4 negmemo validation).
- **R84/R85 handoff SleepMutex** (sync/mutex.rs): waiters/tokens/
  anti-starve state machine, now loom/Kani-verified (see S-1 in
  section A — the verification and the design are one story).
- **D-2 Packed kernel-stack slab** (x86_64, cross-ref D-2): kernel
  stacks carved three-per-256 KB order-6 block (80 KB slots) by a
  dedicated slab with a side-table slot map and empty-block reclaim,
  instead of one power-of-two buddy block per request. Numbers: 85.3
  KB footprint per stack vs 128 KB (−115 MB at the launcher-floor
  population where stacks were 42% of the 904 MB floor); 99.9% slot
  packing live (sc237: kb=981/ks=2941); deepest-ever population boot
  3445 live tasks with zero canary hits; riscv64 suite 1096/1096 with
  the arch gate in place (the ungated variant read 1094/1096 — the
  gate caught it). Guard: R18-2 canary per slot, KSTACK-HW depth
  probe, SC-ANAT packing columns.

## C. Correctness-by-construction, and the parity discipline that earns the divergences

- **Checked UTF-8 on every userspace-writable string** (commits
  f0c57182 / 3ad32b9e): userspace cannot panic the kernel printer —
  the `from_utf8_unchecked` family (10 sites) on task comm/name paths
  was found by a stress boot (lb266: a non-UTF-8 comm panicked the
  KOT-KILL group print at t=209 s) and eliminated; comm prints go
  through a print-safe decoder that cannot panic.
- **Bounded, allocation-free diagnostics**: every instrument is gated
  (one atomic load when off), budgeted, and allocation-free by rule —
  the observer cannot become the pathology. The rule was paid for: the
  bio write-plan fix iteration 1 (see E) allocated from a spinlock
  context and was REVERTED; the allocwatch print-during-hold family
  was bounded after it was caught killing the boots it watched
  (fcfbf949).
- **Reference-parity hunting discipline**: the register's D-numbers
  are where Rux deliberately does NOT match the reference kernel, with
  gates. The earning currency is the opposite class: semantic
  divergences found by source-diffing against the reference tree and
  fixed by ADOPTING its behavior — root causes, not symptoms. A sample
  of this quarter's receipts (all on main):
  - execve returns ETXTBSY while a write-open exists — the
    deny_write_access analogue (e66cfb4d; execve04).
  - SysV IPC objects scoped per ipc_namespace — key visibility and id
    resolution filtered by the caller's namespace (1bde7490; LTP
    msg_comm "messages leak between namespaces").
  - Shared-futex keys resolve to the PHYSICAL word, so cross-address
    wakes after fork+exec reach the waiter (122d8a78; sighold02 /
    waitid08 family).
  - Coherent tmpfs MAP_SHARED mmap through the page cache (799cb942;
    the LTP /dev/shm results-page family).
  - binder poll wake + availability semantics — the never-woken
    unflagged-reader class the reference cannot have (491ebc09; four
    divergences fixed from one 6.6-source diff).
  - Re-enable IRQs at the context-switch boundary — the dead-tick
    poison cycle where kernel continuations resumed at IF=0 and LAPIC
    timer IRQs piled undelivered (d3700224; SP-5). Post-fix,
    exhaustion-class stress runs complete their full budgets: 5.42M
    ops in the 100 s window, zero strike-only kills
    (stress-port/run/stress-runs/findings.md).
  - Also: execveat flags/AT_EMPTY_PATH/ELOOP semantics (5587c257),
    O_DIRECT read flushes the dirty page-cache range first (99ea87e3),
    poll rejects nfds above RLIMIT_NOFILE before allocating
    (f27243a4), poll-family waiters wake at the precise ns deadline +
    timerfd TFD_TIMER_CANCEL_ON_SET (cea11690), SYSRET eligibility
    falls back to iret on TF|RF rflags (3f1b0cd0).

## D. Safety properties (the class mainstream kernels rent with tooling)

- **Memory safety of the kernel interior by construction, measured**:
  the generated TCB ledger (scripts/gen_tcb_ledger.py, snapshot in
  docs/architecture/security-design.md, 2026-11-01) counts 179
  service files of which 47 enforce `forbid(unsafe_code)` at the
  compiler and 56 are already unsafe-free; unsafe tokens: 3158 in
  service code vs 1340 in the Framework layer (the audited interior),
  193 diagnostics, 80 test; SAFETY-comment coverage 3207/5009 (64%)
  with the uncovered set named per module. The verification side runs
  continuously, not aspirationally: 157 Kani harnesses, 1104 host
  proptests, 4 SPIN models, Miri on the verify crate, mirror-sync
  enforced (security-design.md §0).
- **The user/kernel boundary has no identity hole**: MMIO lives
  exclusively behind kernel-half alias windows and the bootstrap
  identity map is unlinked after boot (D-17) — copy_from_user at low
  addresses FAULTS instead of reading physical memory (the prlimit64
  EFAULT gap this closed; x86_64 suite 1194/1194 with the window
  gone). SMAP is enabled with EFLAGS.AC brackets (~55 ns per bracket,
  security-design.md L2) whose semantics hold on hardware AND under
  TCG (D-18); zero SMAP-VIOL beacons across the suite.
- **Panic surfaces enumerated and eliminated as found**: the UTF-8
  printer family (C), the slot-namespace recycler guards, the
  stack-slot recycling guards (D-2's canary + the freed-stack ring).

## E. Performance landed via redesign (numbers, same-rig A/B)

- **vm-op family 15→5 s per CPU-minute under OH load** — see B
  (leaf-table direct unmap walk) for the full number set.
- **SP-7 anon reclaim** (commits 0a778e04 / 5a3878b1): flag-clear
  handouts + head insertion + head rotation — each choice the opposite
  of the pre-fix policy, each attributed by a per-page swap
  skip-reason census. Reclaim examined 35%→82-89% across three reps
  (1,240/3,589 → 2,163/2,629 → 2,990/3,346), swap writes +75%; the
  swap-starvation family closed with verification boots. Source:
  stress-port/run/stress-runs/findings.md (round 7).
- **bio write-plan closure bound** (fs/bio.rs, commit 619dc8cc,
  merged via be56334a): `plan_add_constraints` rescanned every pending
  publication against the whole plan per round — a quadratic the
  reference kernel's block layer cannot grow because it never
  accumulates a constraint plan to close. Decode: 252 identical 5 s
  TXN-HANG samples pinned one task inside `WritePlan::push_entry` for
  a whole 1092 s window (lb229); the KV store-load crawled 550-1100 s
  vs the ~40 s healthy class, 18+ min of kernel CPU inside single
  fsync/mkdir durability points. The landed fix is allocation-free by
  construction — a 3-round cap with a conservative linear fallback
  that schedules a strict superset of the required writes in one pass
  (the index+worklist closure alternative was reverted: it allocated
  from journal/eviction spinlock contexts). Verification state, kept
  honest: make test + tri-arch builds green at land time; the
  store-load re-measure was blocked by the unrelated
  allocator-freeze dark streak of that night — the fix's effect is
  carried by the decode evidence and the fs-family rows below.
- **x86_64 context-switch levers** (commit 9d6980b1 on the gap-lane
  arc): FSGSBASE switch path (rdfsbase/wrfsbase replaces the FS/GS
  MSR pair, measured 393 ns of the per-switch cost) + lazy FPU
  save/restore (the xsaveopt moves to the #NM handler; the kernel
  binary contains no compiler-emitted FP, verified) + a CPUID-gated
  PCID layer (per-mm tags, invlpg shootdowns; this dev host exposes no
  PCID bit, so the A/B delta is FSGSBASE+lazy-FPU, and PCID arms where
  the hardware reports it — the arm-on-every-CPU bug that would have
  #GP'd is itself fixed and boot-verified, af464fb9). Med-of-3
  interleaved A/B: lat_ctx -s64 2: 4.97→2.07 µs (−58%); -s64 8:
  4.68→1.94 µs (−59%); null/read/write guards unchanged.
- **p5 IPC family re-banks** (perf-gate-baseline.md, 2026-11-01
  board, main@afc41b8b): the D-14 measured-wait wakeup-preemption
  gate + idle-CPU steering arc moved p5b_dgram 318.6→23.1 µs (−93%),
  p5c stream 276.5→24.9-26.8 µs (−90-91%), p5c3 32.7→25.0 µs; the
  honest cost side (wake→run behind an incumbent is tick-quantized
  ~10 ms in both hog regimes, both kpreempt settings) is banked in the
  same table as the p2f canary rows — the ledger keeps the cost next
  to the win.
- **fs metadata family re-banks** (same board): stat 200.9→71.1 µs,
  openclose 512.9→156.3 µs, read_cached 546.8→203.7 µs, append
  657.2→77.4 µs, mkdrmdir 22,814→2,757 µs, unlink 1,141→808 µs — the
  D-8/D-12/D-13 design stack (write-through jbd2, batched window fill,
  batched publication + discard-on-free) measured end to end.
- **lmbench KVM standing** (docs/test/lmbench-report.md, standing
  table 2026-11-03, kernel e264a64b, both kernels under the same q35+
  KVM rig): lat_ctx -s64 2 1.98 vs 6.71 µs (3.4x), lat_pipe 5.11 vs
  13.30 µs (2.6x), lat_proc exec 62.9 vs 87.43 µs (1.39x),
  lat_syscall fstat 0.099 vs 0.118 µs (1.19x) — four rows with Rux
  reading lower latency, the rest of the swept rows in the 1.17-2.28x
  band (the read/write/stat/open gaps are attributed to the VFS
  dispatch layer and owned by the perf queue). The whole table, both
  directions, is the artifact — this register records the standing,
  not a selection.

## F. Observability as a first-class subsystem

- **Reference-parity tracefs** (kernel/src/trace/, D-15's
  tracepoints! table): tracepoint table, per-CPU rings, the exact
  set_event grammar, options and snapshot semantics of
  /sys/kernel/tracing. Measured cost discipline: the DISABLED site
  compiles to ONE load + test + branch — verified in release disasm,
  addresses recorded (syscall entry/exit sites in the round-1 build) —
  and ENABLED recording costs +18-19 ns per record (guest
  differential, 4000×8K zero-reads, 3 reps min-of: 957.7-986.7 µs off
  vs 1107.8-1119.2 µs on). Image price +12,288 bytes (+0.12%). The
  migration ledger maps the frozen 59-entry bespoke dfx= table to
  unified replacements; the sched/syscall/binder/kmem waves are
  ported with census A/B verified identical (commit feb54f0a).
  Source: dfx-trace lane TASK.md round-1 verification record
  (run/tr1-kvm-proof-serial.log).
- **Reference-parity tracefs, event catalog 51** (kernel/src/trace/):
  rounds 2-3 grew the table to 51 events across 16 groups — the sched
  waking/wakeup sites and kmem wave (round 2), then the ext4: family
  (sync_file_enter/exit, ext_map_blocks_enter/exit, da_write_begin/
  end, free_blocks — reference ext4.h field rows and print fmts
  verbatim), jbd2:start/end_commit at the journal commit funnel, and
  the rux:epoll documented-divergence family (the reference has no
  epoll events; epollscan/epolltrace bespoke switches alias here).
  Cost discipline held at every wave: all 39 kernel sites verified
  one-load (movzbl+test+jcc) in release disasm; ENABLED recording
  13-19 ns/record (pure-write loop) and 28-35 ns/record (epoll
  ping-pong), round-1's 18-19 ns class; events-off census A/B
  identical (TERM-DESCHED 8/8, WAKE-REFUSED zero-class, 0 panics);
  image price of round 3 +24,576 bytes (+0.182%). The bespoke dfx=
  table stays frozen at 59 and arms reference events through the
  alias arm (whose merge-lost consumer was found and restored in the
  same round — dfx= boots had silently armed nothing).
  Source: dfx-trace3 lane TASK.md round-2/3 verification records
  (run/tr2-kvm-proof-serial.log, run/tr3 round-3 battery).
- **Census + heal: the kernel adjudicates its own concurrency model
  against observed reality** (sched/preempt + interrupt, commits
  1854ec4d / bef10f42 / 8387c00b): every task carries per-field
  tick-observed absence stamps (ti_pc_{p,s,h}irq_zero_jif); a
  preempt-count field PRESENT at 200 consecutive ticks is phantom by
  the five-orders doctrine (every legal bracket is microseconds), a
  field observed ABSENT refreshes its stamp — so lock churn in other
  fields can neither shield a stranded field nor get a live bracket
  false-healed. Paired with the softirq dispatch deferral gate +
  drain budgets + 20 ms starvation bound, this is a self-measuring
  design, not a debugger. Banked numbers (perf-gate-baseline.md,
  pa115debt1, main@afc41b8b): heal counters 64.5k/52.1k heals, 605 ms
  total, model-vs-reality MISS = 0; IF=0 share 25.7% (pa27) → 20.4%
  re-banked; max closed cli segments 594-599 ms one-shots → 6-9 ms
  (back inside the ≤10 ms band); WAKE-STRANDED 233→31 across the
  preempt-hunt arc (OH-lane numbers, same file); strand volume
  17-36/boot → 6/boot (lb21). The known limits are recorded in the
  same place: a holder frozen inside IF=0 is invisible to its own
  CPU's tick — only a cross-CPU observer can name it.
- **allocwatch / lostpark: instruments built to exonerate** (dfx
  family, commits 47dc1901 / 6785a93f / ab756efe / fc04f6d9 /
  fcfbf949): the allocator-freeze hunt produced a cross-CPU holder
  watch (the waiter cannot self-report — printing allocates and it is
  parked IN the allocator), fixed per-key slot tables, a
  section-duration meter that named the cost class (lb268's holder:
  ONE bounded 300 ms section, buddy-word clean), and print discipline
  that never kills the boot it observes. The verdict the instruments
  delivered is the register-worthy part: the claimed freeze did NOT
  reproduce on the quiet host — an instrument mirage decoded and
  recorded (fourteenth-successor close, da27be84), and the lostpark
  hypothesis adjudicated userspace-internal (f6848f7c). Negative
  results with receipts are part of the evidence discipline.

## G. Pending evidence slots (placeholders until measured)

- **S-2 typed binder transaction FSM** (survey shortlist item,
  scheduled with the OH lane): binder.rs transaction states modeled
  as an enum FSM so illegal state transitions are unrepresentable —
  PLACEHOLDER; no A/B numbers exist yet.
- **S-3 anykernel-style in-host test slice for ext4/jbd2** —
  PLACEHOLDER; depends on none, sized 1-2 sessions in the survey.
- **S-4 internal completion-ring writeback substrate** — PLACEHOLDER;
  depends on S-3's replay scenarios.
- Standing rule for this section: an item leaves G only by landing a
  measured row in A-F, in the change that ships the design.
