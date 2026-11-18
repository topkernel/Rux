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

- **TLB-flush epoch ack for deferred page-table frees** (x86_64 ipi.rs
  + mmu_init.rs, fork-frag lane 2026-11-17, commit 746678591): Rux's
  shootdown IPI is fire-and-forget by design (zero per-page state, no
  ack handshake), which left munmap no ordering proof to free an
  emptied leaf table against. The protocol adds one per-CPU counter
  incremented at the END of the flush handler — an epoch advance
  proves that CPU completed a FULL non-global flush after a recorded
  snapshot, and any advance counts (the handler flushes everything,
  not a range) — so a freed table frame waits on exactly the CPUs
  that ever loaded the mm, with zero added IPI traffic and zero
  change to the shootdown fast path. The reference needs
  smp_call_function wait states and RCU-batched tlb_remove_table for
  the same proof; here the proof is two atomic loads per drain. The
  dividend it unlocked: munmap frees emptied leaf tables (fragmented
  parents carried 36 tables against 61 present PTEs), collapsing the
  fork/teardown table churn — pbench SMP-8 KVM p4_fork_exit
  68.6→39.0 µs and p4_fork_exec 77.2→44.3 µs vs reference
  12.9/25.7 (5.31x→3.02x / 3.00x→1.72x), forkprof 36.0→7.0 leaf
  tables per fork; clean small-parent shape flat (lmbench lat_proc
  fork 46.3→45.2 µs interleaved med). Evidence: run/FORK-FRAG.md.
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
  - GUP-pinned user-page DIO (gup-dio lane, 2026-11-18): the reference's
    pin_user_pages / bio_iov_iter_get_pages shape — mm/gup.rs walks the
    current task's page tables under the mm PTE lock (INV-LOCK-7) after
    fault-in (FOLL_WRITE discipline for BOTH DMA directions), takes a
    page refcount per frame (reclaim's refcount==1 gate keeps pinned
    frames DMA-safe); the ext4 direct reader/writer build their SG
    chains straight from the frames' direct-map aliases (physically
    adjacent pages merged into one segment; chains sized by the
    negotiated virtqueue seg_max, descriptor-budget waves), unpinnable
    pages become typed holes served by the staged paths. Same-rig
    kernel-vs-kernel (q35 KVM smp8/8G, 48 MiB psync 1M direct, clean
    window): GUP 8000 MiB/s clat 109 µs vs the same tree's staging
    path 6000 MiB/s / 145 µs — the full-request staging copy and the
    copyout/copyin terms are gone (pin 10 µs/MiB); fio --verify=md5
    direct 1M/64k/4k + unaligned-offset heads err=0; pbench Tier-2
    rows in band or improved; riscv make test 1334 PASS.
    Evidence: /home/william/rux-agents/gup-dio/run/GUP-DIO.md.
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
- **The unified-track vDSO: one timekeeping anchor serves kernel and
  userspace bit-identically** (vdso-rearm lane, commit 4c0cabe85,
  2026-11-15): the x86_64 timekeeping anchor {cycle_last,
  base_mono_nom, mult} lives in the vDSO data page itself; the
  kernel's `read_time()` and the hand-emitted AMD64 vDSO functions
  run the SAME seq protocol (seq1 → fields → lfence;rdtsc → seq2) and
  the SAME integer arithmetic over the SAME words, so a syscall clock
  read and a vDSO read at the same instant agree bit for bit. That
  structural identity is what LTP clock_gettime04 demands (strict
  non-negative deltas between ADJACENT vdso and syscall reads —
  10,000 interleaved pairs per clock) and it is achieved by
  construction, not by tolerance tuning: the reference kernel keeps
  separate kernel/vvar clockstate and accepts cross-track skew that
  its own LTP test tolerates through coarser pair spacing; here the
  two tracks CANNOT diverge because they are one track. The seq word
  doubles as the writer lock (cmpxchg even→odd), closing a multi-
  writer tear class (two tick CPUs racing a fetch_or publication).
  Measured, name-only same-rig A/B (OH boot, tok-r4fix vehicle, KVM
  q35 SMP2 4G, syscost, 780 s, kernels differ only in the two dynstr
  symbol names): clock_gettime syscalls 8,259,773 → 0 per boot
  (−29.4 CPU-s = −2.0% of the box, −26% total syscall volume; the
  biggest burner OS_FFRT_2_0's 1.1 M calls all move to userspace),
  with nanosleep avg (155.1 vs 149.7 ms) and the GRQ lock-site avg
  (4.97 vs 5.44 µs) flat, zero panics both arms. Functional:
  clock_gettime04 6/6 incl. both COARSE clocks and the vdso variants
  (the exact test that forced the 106th's disarm), 480k probe
  adjacent-pair checks zero negatives. Source: vdso-rearm lane
  run/VDSO-REARM.md + run/pa-vdsorearm-{a,b}-serial.log.

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

## H. OH bring-up campaign additions (2026-11-10..11, evidence-backed)

- **REMOTE-BT: live remote stack capture as an instrument primitive**
  (4950f396): when every CPU spins on one heap-object lock, naming
  the HOLDER requires reading a stack that is not yours — this port
  does it over the IRQ_WORK IPI: the poked CPU's handler stores its
  interrupted pt_regs plus an fp-walk into a fixed per-CPU slot, the
  requester polls lock-free and prints raw addresses (zero
  allocation in either path — the box is by definition at its memory
  limit when you need this). Proven on its first live boot: named
  the vma-lock Rwsem wait_lock inside an MmStruct and its owner's
  munmap → mm_surgery stack — which produced the 91st's spin→Rwsem
  fix and the first-ever pristine-boot launcher spawn (63.9s, then
  46.2s class). This is not a port of a reference feature; the
  reference has no equivalent one-shot remote-stack tool at this
  granularity.
- **Crash-window data-integrity rigs as a methodology** (fs-integrity
  lane, fi3/fi4/fi5/fi7/fi8/fi9/fiA-D): the sync(2) durability hole
  was chased with two-boot A/B (write+sync+hard-reset, then a fresh
  boot reading back with md5) plus HOST-SIDE raw-grep of the crashed
  image for a known pattern — the combination distinguishes "data
  never reached the device" (grep=0) from "data on disk, metadata
  lost" (grep=2.6M, file size 0) without trusting the guest. The
  FI-* bounded instrument family (drop/release/census/collect/
  icache-miss/markdirty-miss prints, each rate-limited, each with a
  backtrace hook) is the reusable template.
- **hiperf as a portability oracle** (e5f1536f + 2cf784d1 + 970b1b40):
  building the reference's own profiler against our perf_event_open
  exposed parity gaps no synthetic test had (inherit semantics on
  per-CPU counting events; O_RDWR on the ring fd for MAP_SHARED+
  PROT_WRITE; MISC cpumode byte order) — and once fixed, delivered
  the campaign's hotspot tables (80,961 → 244,516 samples, kernel
  1-5 / user 2-16+ frame chains). Instruments that ARE the workload
  beat instruments that approximate it.

## I. Merge-day additions (2026-11-14, evidence-backed)

- **Per-hop input verdict tables (INEV/INOP/INRD)** (wake-drop merge
  f4c57e7c6 carrying the oh-input-chain tracepoints
  f23d4266a/bb798852c; events input:input_event/evdev_read/
  evdev_open, ids 53-55): the question "where does pointer input die
  between kernel evdev and the OH UI?" was answered hop-by-hop with
  a PASS/PARTIAL/FAIL verdict per hop, each backed by counted
  tracepoint output on the serial console (the OH rig cannot mount
  tracefs in-guest): (i) injection fidelity PASS — 149/112/106 INEV
  records per boot with every tap coordinate landing exactly
  (tap x=360 → `code=0 value=16406` on the 720-wide panel, 32
  BTN_LEFT press+release pairs, 46 SYN, KEY_UP/KEY_ESC on the
  keyboard node); (ii-a) open PARTIAL — only audio_server ever opens
  input nodes (event1+event2, never event0), multimodalinput ZERO
  INOP on every boot; (ii-b) read FAIL — exactly 2 reads per boot,
  both the first payload event of the first gesture, then zero reads
  across ~100 queued events; (iii) dispatch ZERO (moot); UI reaction
  NONE with the same injection on the reference kernel reacting
  (control proves the method). Result: the kernel evdev path
  exonerated hop-by-hop; the break mapped to mmi's device-manager
  init never completing (image-side service chain; its launcher
  transactions run 5.0-8.6 s) plus the WAKE-RUN-LEAK scheduler
  family downstream of the one stranded reader. The methodology —
  per-hop identity (pid/tgid/node) at one load+test+branch per site
  when disarmed, budgeted serial echo when armed — is the reusable
  artifact; a synthetic probe cannot deliver a per-hop verdict
  because the verdict IS the counted absence on the real image, and
  the reference kernel has no built-in per-hop input identity to
  count against. Source: oh-input-chain lane
  run/INPUT-CHAIN-FINDINGS.md +
  run/serial-boot1/5/6.log, run/inj*/.
- **Frozen-user-word forensics: the kernel adjudicates USERSPACE lock
  state** (rwlock-livelock merge 95b5e61fb, commits ac33690d2/
  1d0ed59ab; census + op-watch instruments, all default-off): when
  Ubuntu 24.04 froze mid-boot, the suspect was our futex/scheduler
  machinery. The instrument family read the frozen word from inside
  the kernel and decoded it against libc semantics: RWFRZ-MEM dumps
  the lock-struct neighborhood (word=2, +0x8 __owner=0, +0x10
  __kind=0 — a glibc NORMAL mutex, not the suspected rwlock; the
  old census's "readers=2" decode was a musl-shape heuristic misread,
  now corrected), APPPARK-RETS captures the waiter's USER return
  chain call-site-validated and symbolized against the image's own
  libs (futex syscall ← g_cond_wait+0x4d ← g_async_queue_pop/
  push_sorted ← udisksd frames — naming GLib's gthreadpool spawn
  queue as the subject), and the op-watch twin records every futex
  op on the word from t=0. Measured verdict across 6 boots
  (F/G/J/K/L/M): the complete word lifecycle is a 5-op sequence
  (boot L #0-#8; the terminal shape is word=2 — glibc's waiters bit,
  where every future unlock MUST futex_wake), after which the word
  is silent FOREVER (zero ops in every boot, watch pre-armed), and
  the one pre-freeze WAKE(val=1, ret=1) was DELIVERED (the woken
  waiter's WAIT returned ret=0 immediately). Kernel exculpated at
  word level: no lost wake (none was issued), no kernel write to
  user futex words on WAIT/WAKE paths, no runnable task stranded
  (tgid dumps show all parked). This is the allocwatch/lospark
  "built to exonerate" tradition (F) extended across the user/kernel
  boundary — an in-kernel decoder for libc lock objects that the
  reference kernel has no equivalent of at this granularity.
  Source: rwlock-livelock lane run/LIVELOCK-FINDINGS.md +
  run/serial-F..M.log, run/imgx2/ (symbolization inputs).
- **Line-atomic dump staging under the printk emit lock**
  (obs-hygiene merge 8f4a4a259, commits 3f87a9ee6/a77685745): hunt
  dumps (taskdump, WQLEAK, census, trap printers) were per-byte raw
  UART writes — on SMP8 OH boots the concurrent /dev/kmsg flood
  (printk records holding the console-emit lock 0.4-17 ms per line)
  spliced dump bytes into flood records and vice versa, destroying
  115/6783 task lines (1.7%) including every line the hunt depended
  on. The landed design stages each line in a PER-CPU buffer and
  emits it as one batched TX pass under printk's console-emit lock,
  with a BOUNDED acquire (Full 50 ms, irqs-on, for dumps; Brief
  100 µs for GRQ-held and IRQ-context hunt prints) and a lockless
  fallback that keeps the readable-at-any-cost contract. Measured:
  mangled task lines 1.7% → 0 (1506/1506 well-formed, r2), ENQ hunt
  lines 240/240 whole (previously char-mangled); the two design
  defects the first cuts had were caught by the smoke boots
  themselves and fixed in-round (shared stage byte-mixed → per-CPU
  stages; IRQs-off emit wait tripped the RCU frozen-CPU detector →
  stack-copy + flush with caller's interrupt state). Honest
  residuals recorded: same-CPU nested dumps can still mix (a
  whole-dump mutex is not survivable from trap/NMI), and brief-budget
  fallbacks can append line-granular onto a long kmsg record tail
  (14/238 in r2 — whole line visible, no char mixing); the
  deferred-ring follow-up that kills that class is specified in the
  lane record. Crash-context writers (trap printers, oops, spinlock
  watchdog) deliberately stay raw. Not a port: the design couples
  the dump emitter to the printk console-emit lock with a bounded
  acquire and graded budgets — the reference kernel serializes
  printk records against each other but has no equivalent contract
  for out-of-printk diagnostic dumps, which is exactly the class of
  writer a hunt kernel needs atomic. Source: obs-hygiene lane
  run/OBS-HYGIENE.md + oh-before/r1/r2/r3-serial.log.

## J. NOHZ adaptive tick (2026-11-16, evidence-backed)

`kernel/src/timer/nohz.rs` mirrors the reference's kernel/time/tick-sched.c
function-for-function (`check_tick_dependency`, `tick_nohz_dep_*`,
`sched_can_stop_tick`, `tick_nohz_next_event`, `tick_nohz_stop_tick`,
`tick_nohz_stop_sched_tick`, `tick_nohz_restart_sched_tick`,
`tick_nohz_full_update_tick`, `tick_nohz_irq_exit`, the `nohz_full=`
cpulist parameter). Structural mappings, each a consequence of a known
tree divergence — not new mechanisms:

- per-rq -> the global run queue (D-1): the SCHED tick dependency is
  evaluated on the fly by `can_stop_full_tick` (the reference caches it
  in a per-cpu bit maintained by `sched_update_tick_dependency` from
  add_nr_running/sub_nr_running; the 0->1 kick side is kept as a
  resched-IPI to stopped CPUs from the enqueue edge).
- `rq->nr_running` includes the running current (a task stays on_rq
  while it runs); Rux's GRQ counter drops the current at pick, so both
  consumers (`sched_can_stop_tick`, the `entity_tick` guard) add the
  running non-idle currents back — `cfs_rq->nr_running > 1` maps to
  `timeline_count > 0`.
- clockevent KTIME_MAX arm -> `timer_arm_oneshot(u64::MAX)`, whose u32
  count clamp parks the LAPIC at its maximum delta (the reference's
  clockevent programming clamps the same way).
- tick_do_timer_cpu -> the boot CPU with the sleep capped at
  TIMEKEEPING_MAX_DEFERMENT (jiffies advance from the timebase inside
  the tick handler, so some CPU must keep waking for
  increment_jiffies_from_timebase).
- perf: the reference sets TICK_DEP_BIT_PERF_EVENTS for freq-mode
  events only; Rux's software sampling rides the tick (the reference's
  rides per-event hrtimers), so the dep tracks any armed sampling
  event. Documented divergence; the reference's per-event-hrtimer
  swevent is the follow-up that removes it.
- Rux's idle loop calls schedule() every pass, so the idle-section
  bracketing of the reference's cpuidle loop collapses into the same
  `can_stop_full_tick` machinery at every IRQ exit
  (`tick_nohz_irq_exit`); non-nohz_full CPUs keep the plain grid-glue
  arm (pre-nohz behavior, byte-for-byte).

Companion scheduler change (same commit): the GRQ slice check now
carries the reference's `entity_tick` guard (`cfs_rq->nr_running > 1`)
— a solo CPU-bound task is no longer dequeued/requeued/re-picked on
every tick — and the ENQ-CLAIMED-SLOTBK print (one ~1 ms polled-UART
line per tick-preempt on a CPU-bound task) is deleted; the perf-sample
capture of the strand hunt (2026-11-15) showed it carried no signal.

Measured (same rig, same instant, 4+4 interleaved lmbench
`lat_syscall,null` boots, runner budget 35 s, hostile background of 6
foreign QEMUs):

- main tip (pre-everything): 0/10 PASS under load (10/10 TIMEOUT).
- churn fix only (nohz_full absent): 5/12 PASS (42%).
- nohz_full=1: 11/12 PASS (92%).
- pbench `p1_null` median: 0.050 us (nohz_full=1) vs 0.060 us (off),
  -17%; the off config equals main tip's own number (0.051) — no
  measurement-quality loss.
- pbench boot completes with `nohz_full=1` (rc=0, DONE at t=54 s;
  0 WAKE-STRANDED lines) and without (t=36 s, 0 lines) — the pre-nohz
  kernel stranded pid1 for 27-30 s stretches (216M pick walks) in the
  same phases.
- Tier-1 census bands hold exactly under nohz_full=1: TERM-DESCHED 8
  (band 8+-2), WAKE-REFUSED 32 (band 32+-4); GRQ avg hold 699 ns vs the
  1200-7320 ns band (improvement, rebank); small_alloc REST 0.0%.
- Gates: riscv64 `make test` green (unit suite verdict green); aarch64
  builds clean; dual-arch x86_64 debug + release-opt build green.

## K. Batched direct I/O with borrowed DMA (fio-2x lane, 2026-11-17, evidence-backed)

- **Run-resolved, window-batched O_DIRECT with a borrowed DMA source**
  (merge candidate 5606622b8 + 73f0ee509; ext4 file/extent, bio): the
  direct reader resolves a request into physically-consecutive runs
  with one extent-tree walk per run (`ext4_ext_get_run` — the maximal
  (phys, len) pair, replacing 1-2 cached breads per 4 KiB), then DMAs
  each run as ONE multi-block request straight into the syscall's
  staging slice — no private kernel copy — keeping a window of up to
  64 requests / 512 blocks in flight, waited once, one device kick
  per window. The write side's fast arm is the Rust-native core:
  `WriteBatch::submit_ref` lends the CALLER's buffer to the device as
  the DMA source and the borrow checker enforces the discipline C
  kernels hand-roll — the buffer cannot be mutated or dropped while
  the device owns it, because the drain happens before return inside
  the same lifetime. In C this is refcounted page pinning with a
  use-after-unpin bug class; here the class has no representation.
  Measured (fio 3.36 static, q35 KVM smp8/8G, same 4 GiB ext4 data
  disk both arms, snapshot=on, 3 interleaved reps, medians; artifacts
  run/ab-final3-*.txt, run/FIO-2X.md): 1M seq read direct 40.8 →
  4711 MiB/s vs reference 9600 (226x-behind → 2.0x); 1M seq write
  direct 93.9 → 1583 vs 4019 (57x → 2.5x); the 4k-random-direct
  write storm (166 IOPS, 4 ms/op) was root-caused to the extent-decode
  bug below and reads 10.9k IOPS after the fix. Remaining gap is
  per-op completion latency, not batching (queue item 4, wakeup
  delivery).
- **Batched journal write-through with one drain per window**
  (jbd2/bio, same merge): both jbd2 commit write-through loops and
  the fsync data walk go through `bio::sync_dirty_blocks_batched`
  (pin, submit 32-span windows, ONE device drain, clear BH_Dirty on
  ack) instead of one serialized round trip per registered buffer,
  and the fsync walk probes dirtiness with a no-pin lookup
  (`block_dirty_cached`) instead of pinning every cached block of the
  file (measured 59.3M breads in 15 s on the old walk; 2048 hash+LRU
  pins per fsync of an 8 MiB file). 4k randwrite+fsync=1: 260.6 →
  2727 IOPS vs reference 15.8k (7.7x → 5.8x behind; the residual is
  per-fsync write-core cost, tracked in the lane record).
- **The decode-bug class eliminated by ONE accessor**: ee_len==0x8000
  is the on-disk encoding of a max WRITTEN extent (32768 blocks);
  every decode site inlined its own mask, and several read bit 15 as
  the unwritten marker — every >=128 MiB contiguous file read back as
  ZEROS (silent, in-bounds, no memory corruption — a class-(b) logic
  bug in safe code, caught by in-guest md5sum vs host digest, not by
  the type system; honest note in the safety ledger). All sites now
  share `Ext4Extent::length` / `is_unwritten`, so the divergent-decode
  family cannot re-emerge at a new call site — the accessor is the
  single place the on-disk discipline lives.

## L. Incremental ext4 readdir via a typed fill op (getdents lane, 2026-11-17, evidence-backed)

- **`INodeOps::readdir_fill` — the reference's filldir loop as a Rust
  contract.** Linux's ext4_readdir walks directory blocks one at a
  time and copies each dirent into the user buffer through filldir;
  the Vec-returning `readdir` op shape (materialize the whole
  directory, then filter by index) made every fresh-fd full-dir sweep
  pay O(entries) heap work — 1000-entry sweep at 1374 µs vs the
  reference's 72 µs. The new op is a typed contract instead of a
  C-style callback: `fn(&Inode, pos: u64, buf: &mut [u8]) ->
  Result<(bytes_filled, next_pos), i32>` with the record format,
  every-byte-written (pad included), EOF/EINVAL and sleep rules
  documented at the type (kernel/src/fs/inode.rs). The VFS branch is
  8 lines; ext4 owns the walk.
- **Byte-offset f_pos, d_off = resume point.** The file position
  becomes the ext4 directory byte offset and each record's d_off
  names the next entry's offset — telldir/seekdir round-trip through
  plain lseek with no index translation (the old scheme was an index
  into a synthesized Vec, meaningless across calls). "." and ".." are
  emitted from the REAL on-disk entries (true inode numbers) instead
  of being re-synthesized per materialization.
- Measured (pbench dual-kernel rig, q35 KVM -cpu max smp8/2G,
  1000-entry ext4 dir, 32 KiB buffer, n=1000 sweeps; interleaved
  3 base + 3 head + 2 reference boots): 1374.3 -> 37.0 µs vs
  reference 7.0.0-38-generic 72.2 µs — 19.0x behind -> 1.95x ahead.
  Guard rows same boots: p6_stat 0.281/0.281, p6_openclose
  0.681/0.691, p6_read_cached 0.982/0.992, p1_null 0.050/0.050.
  Functional semantics probe (test/getdents_probe.c, 15 cases: dot
  entries + ino truth, d_off monotonicity, restart determinism,
  seekdir resume with zero loss/dup, mid-walk unlink, EOF, EINVAL on
  1-byte buffer, 4096-entry dir, glibc DIR* walk): 15/15 PASS.
  Artifacts: run/GETDENTS.md, run/pb-gdab-*.log, run/pb-gdref-*.log,
  run/pb-gdprobe-h3.log.

## M. Close-path conditional refcount acquisition (oc-rc lane, 2026-11-17, evidence-backed)

- **The unconditional-increment fd lookup forced every close(2) to
  wait out an RCU grace period; conditional acquisition moved the
  guarantee into the refcount transition itself.** The old protocol
  (fs/fdreap.rs, pre-oc-rc): `get_file` cloned a slot's Arc with an
  UNCONDITIONAL `increment_strong_count`, so a closer could only prove
  "no in-flight reader will bump the count after my observation" by
  parking the Arc and waiting a full grace period before the close
  decision — ~250-330 ns charged to every close(2), plus a wakeup/IPI
  path whenever the reaper raced the closer. The rework splits the
  reference's fput shape into its two halves: (1) the close DECISIONS
  (record locks, flock/OFD/lease, fsnotify, ops.close) run at the
  last strong reference in `File::drop` — the closer's own context on
  the common single-fd close, exactly one drop-to-zero so the op runs
  exactly once, with no grace observation needed because a racing
  reader's CAS from a nonzero count LEGITIMATELY KEEPS the
  description alive (reference fget-holder semantics); (2) the
  ArcInner MEMORY is pinned by a parked Weak and freed by fdreapd
  after an asynchronous grace (the reference's call_rcu(file_free_rcu)
  shape). The reader-side CAS (`file_arc_try_clone`) is the
  reference's `get_file_rcu`/`atomic_long_inc_not_zero` shape, with
  the std-Arc ArcInner strong-word offset pinned at boot
  (`verify_arc_inner_layout` constructs, clones, downgrades and drops
  a probe Arc and asserts every transition) — a toolchain move of the
  refcount word fails loudly at boot instead of CASing wrong memory.
- **The same round embedded the read-ahead state in File** (the
  reference's `file_ra_state` inside `struct file`): the Box in
  private_data paid a slab alloc + free pair per open-then-read cycle
  and needed a per-fs close op to free; the field dies with the
  description and ext4's close slot disappeared (capability matrix
  updated deliberately, kernel/src/tests/fileops_caps.rs).
- **Read path**: page-cache get/put now share ONE claimed entry
  (put_range_with_entry) — the release-side bank-claim + probe +
  strong-count round trip was ~half the pin+unpin cost of a 1-page
  hit; per-page STAT_HITS RMW hoisted to one per call (same totals).
  atime stamping gated by the reference-default relatime rule against
  a tick-cached wall second (one relaxed load; LTP utime keeps the
  exact wall_secs form).
- Measured (pbench dual-kernel rig, q35 KVM -cpu max smp8/2G, n=10000,
  med-of-3 interleaved boots vs same-rig baseline boots):
  p6_openclose 0.681/0.691 -> 0.591 µs (3/3 boots 0.591; reference
  0.371), p6_read_cached 1.002/1.012 -> 0.902/0.912/1.022 µs
  (median 0.912; reference 0.551). Decomposition (rdtsc probe,
  run/diag_oc.c): close side 0.220 -> 0.120-0.140 µs, warm read term
  0.30 -> 0.24-0.25 µs, warm open+read+close 0.811 -> 0.75-0.77 µs.
  Guard rows same boots: p6_stat 0.281 flat, p6_getdents 33.8-34.1
  vs 34.5-35.0 baseline, p6_append 0.631-0.661 vs 0.621-0.631
  (noise band), p1_null 0.050 flat. riscv64 make test 1328/1328.
  Artifacts: run/OC-RC.md, run/pb-s8oc-base1.log, run/pb-ocrc3.log,
  run/pb-ocrc-f*.log (lane worktree /home/william/rux-agents/oc-rc).
- Honest floor statement: both rows remain 1.59-1.66x behind the
  reference. The dominant remaining term is the OPEN-side path walk
  (~0.24 µs of the 0.59; the p6_stat row shows the shared walk at
  1.28x behind on its own) — stat-lane territory, not close-path.

## N. Poll-before-sleep completions for synchronous I/O (submit-2x lane, 2026-11-17, evidence-backed)

**What is Rust-native here:** the completion object is a plain kernel
value (`IoCompletion`: three atomics + a wait-queue head), so EVERY
synchronous wait can poll it directly before parking — no per-wait
plumbing, no scheduler hooks, no special poll mode in each driver.
`IoCompletion::wait()` polls its own `done` flag under a bounded
pause-loop budget (1536 iterations ≈ 14 µs on the 3.7 GHz KVM rig)
before arming the 10 s deadline timer and scheduling out. IRQs and
preemption stay armed: the virtio-blk MSI-X hardirq interrupts the
loop, the completion walker retires the chain, and the next polled
load observes `done` — the wake, timer arm/disarm, schedule-out/in
and wait-queue teardown are skipped entirely for completions that
land inside the budget (the whole sleep path costs more than the
device round trip itself under KVM). The reference kernel makes the
same trade only in opt-in modes (io_uring HIPRI / nvme poll queues);
here it is the default for every synchronous block-I/O wait, which
is safe because the budget is bounded and the waiter, at depth 1,
has nothing else to run.
- Companion shape (same lane): the DIO write batch carries its
  completions in inline slots re-armed per flush window (bio.rs
  `WriteBatch`) — zero per-span heap traffic, mirroring the direct
  read batch's stack array; and the virtio-blk 64-byte header+resp
  blocks come from per-CPU free lists (`io_buf_alloc/io_buf_free`)
  instead of the global allocator on every chain submit/complete.
- Measured (same-window kernel-vs-kernel interleave, 4 boards
  alternating, q35 KVM -cpu host 8G smp8, host load 10-15):
  4k randrw direct 14.95k -> 22.15k IOPS/side (+48%, reference
  parity: REF 22.0k same rig); 4k randwrite direct 29.5k -> 36.75k
  (+25%); 4k randread direct 31.85k -> 37.0k (+16%); 1M seq read
  direct +6%. fio --verify=md5 clean (4k buffered, 64k direct).
  The A+B+D2 bookkeeping levers alone measured within noise
  (+0.9..+2.6% same-window) — the poll is the lever that moved the
  rows.
- Negative result kept for the record: MSI destination steering
  (rewriting the virtio-blk MSI-X entry to the submitting CPU,
  rate-limited) collapsed rw4k to 107 IOPS under KVM/irqfd —
  completion delivery breaks after a runtime table rewrite in this
  QEMU; reverted, and with polling in place the completion IRQ's
  landing CPU no longer matters for depth-1 waits anyway.
- Artifacts: run/SUBMIT-2X.md, run/kk-*-{kk1,e1}.txt,
  run/ab-*-final.txt.

## O. MSI-X completions walked inline in the hardirq vector (dio-conc lane, 2026-11-17, backfilled from run/DIO-CONC.md)

- The per-op cost was interrupt DELIVERY, not locking: phase probes
  put kick→INTx dispatch at 39.5 µs (QEMU completion + line assert +
  delivery) and the shared INTx dispatcher ISR at 16.3 µs more — it
  polled EVERY registered virtio function's ISR cap, each poll a
  userspace MMIO round trip under KVM. MSI-X per-function vectors with
  the completion walker inlined in the hardirq vector (the reference
  vring IRQ shape) removed both terms: 4k randread direct 11.7–11.9k →
  33.4k IOPS (0.27× → 0.77× of reference, which reads 43.7–44.3k).
  Cross-check that convicts INTx as the mechanism: the REFERENCE
  booted with pci=nomsi drops to 25.1k IOPS.
- Same change carried whole-MiB staging + insert_dirty_batch for
  buffered writes: 6000 → 8000 MiB/s (0.62× → 0.83×).
- Artifacts: run/DIO-CONC.md (segment table + pci=nomsi cross-check).

## P. Adaptive lock-free dentry child memo (stat-2x lane, 2026-11-17, backfilled from run/STAT-2X.md)

- The 4-slot inline child memo thrashed to ~0% hit rate in real
  directories — every lookup then paid the children lock + a BTreeMap
  walk (~10 string compares) + a memo refill. Replaced with a
  self-describing seqlock table sized next_pow2(2×children) clamped
  [128, 2048], two lock-free tiers in front of the children lock
  (inline memo → seqlock table → lock). p6_stat 0.491 → 0.281 µs
  (2.23× behind → 1.28× vs reference 0.220), allocation threshold 12
  children, memory 8 KB (small dirs) to 32 KB (large).
- Companion levers same lane: has_mount() early-out in follow_mount;
  per-inode open counters replacing the global INODE_OPENS table
  (Linux parity — no global table walk per open).
- Artifacts: run/STAT-2X.md (per-lever A/B, thrash experiment).

## Q. ext4 fsync path rebuilt in reference shapes (wrfsync lane, 2026-11-17, backfilled from run/WRFSYNC.md)

- Attribution first: the journal-commit theory was FALSIFIED (/data
  mounts journal-less — zero commits ran). The real per-fsync cost
  decomposed as per-4KiB unwritten-extent materialization (75 µs), an
  O(file) dirty walk probing 2014 clean blocks (80 µs), and two
  serialized ~30 µs device round trips. Four reference-shaped fixes:
  ext4_ext_convert_unwritten-style BULK conversion with neighbor merge
  (one tree edit per write span, node-local no-alloc prescan); the
  tagged-page-tree analog — per-inode dirty-block notes so fsync
  walks O(dirty); the inode-table block riding the same SpanFlusher
  window (one drain per fsync, jbd2-style byte-snapshot settle guard
  keeping BH_Dirty on raced shared blocks); binary-search extent
  leaves (was O(entries) linear per resolve).
- Measured: fsync4k 4653 → 20.1–22k IOPS vs reference 15.3–16.2k
  (0.30× → ~1.24× AHEAD — the first storage row ahead); seqw1m_d
  1583 → 4000 MiB/s (0.39× → 0.83×; root cause found: fio's
  fallocated layout made the extent resolver return phys=0 so the DIO
  fast run never fired). fio --verify=md5 clean on every build;
  in-guest md5sums match host digests.
- Artifacts: run/WRFSYNC.md (TSC decomposition, per-lever A/B).

## R — RT kernel stack (real-time capability, 2026-11-17/18)

The RT plan (docs/development/rt-kernel-plan.md) landed a staged real-time
capability on Rux's own structures, NOT the PREEMPT_RT full-tree conversion
(recorded divergence: Rux's measured critical sections are small — GRQ actual
hold ~102 ns — and nesting is a compile fact via D-11 typed lock keys; we
shrink and bound sections instead of making them sleep, and convert only the
locks that need PI semantics).

Measured evidence (same-image same-binary pair battery, docs/test/
pair-battery-2026-11-17.md):
- **R3** (D-14 measured-wait gate batch window 6 ms → 0.7 ms wakeup
  granularity): khog futex wake→run median **28,135 → 150 µs (187×)**; the
  D-14 ping-pong win preserved (p5b_dgram 43–48 µs flat, p1_null/p2 flat).
- **R1** (irq-exit kernel preemption default): OH SA-storm release boot
  600 s zero DEADLOCK prints (the documented re-land condition); cyclictest
  OTHER max 29,061 → 2,285 µs (12.7×) on the first armed boot.
- **R5** (forced per-IRQ threading, irqthread=1): 4/4 canary boots complete
  with virtio completions on a SCHED_FIFO 50 kthread; khog med 157–208 µs
  held — threading did not degrade the wake path.
- **R6 LANDED** (rt_mutex real PI chains, rt-r6 lane 2026-11-18): the
  boost now propagates NUMERICALLY along the chain —
  rt_mutex_adjust_prio_chain's walk re-implemented as a pool-re-scan
  walk holding one bucket at a time (D-27) with LIVE top-waiter
  evaluation — plus rt semantics on the sleepable locks RT tasks can
  block on (D-26: VFS-MUTATION/EXT4-BIGLOCK/loop-io via
  SleepMutex::new_rt, kernel-keyed PI states in the same chain
  engine), the glibc PTHREAD_PRIO_INHERIT robust UAPI chain closed
  (LOCK_PI returns 0 with FUTEX_OWNER_DIED latched in the word — the
  reference contract; glibc's fatal-error switch rejects
  -EOWNERDEAD), and the owner-side cycle-detection fix
  (rt_mutex_owner(lock) == top_task shape). Evidence: pi_chain
  3-deep CFS→FIFO-90 conversion through 3 lock edges 1000/1000
  rounds (300/300 post-rebase, worst top-waiter wait 374 µs);
  pi_glibc robust PI (in-process pthread_exit + cross-process SIGKILL
  death → EOWNERDEAD → pthread_mutex_consistent recovery) 3/3;
  pi_stress -g 2 sustained 273k-383k across runs; pi_min 0/1/2
  200/200 each; LTP futex family TPASS; INV-C1..C5 chain-invariant
  model (kernel/verify pi_model) + INV-M1..M6 still green (1137
  host tests); riscv make test ALL PASS, triple-arch rc=0.

The reference-column comparison (cyclictest FIFO avg 12 µs vs Rux 46 µs) is
the open gap — the fire-late/hrtimer-re-arm attribution is queued (rt-r4).

## R. Entity-embedded intrusive rb-tree CFS timeline (grq-10ns lane, 2026-11-17, evidence-backed)

- **The global queue's timeline stopped being a data structure with a
  heap behind it.** The CFS timeline was a `BTreeMap<VruntimeKey,
  *mut Task>` — every enqueue and pick descended the tree under the
  GRQ lock, and a B-tree split/merge could call the ALLOCATOR inside
  the critical section. Replaced with an entity-embedded intrusive
  red-black tree carrying a cached leftmost (the reference
  `rb_root_cached` shape): the node lives inside
  `SchedEntity::run_node`, mutations are pointer rewires, and the lock
  holds ZERO allocations by construction (the BTreeMap import is
  gone from fair.rs).
- **The pick's rejection handling was the real tail.** The old pick
  stashed refused candidates and re-did the walk (2×O(log n) per
  refused candidate) and initialized a 768-byte stack array on every
  pick inside the lock; the successor walk steps past refusals IN
  PLACE. Timeline mutations per boot collapsed 19,785,398 → 145,524
  (−99.3%, 2.0 → 0.013 per schedule) and the p2_ctxsw p99 bimodal
  434–486 µs tail (a genuine stash-churn contention mode) is GONE
  (4.00–4.11 µs stable).
- **Measured (release, q35 KVM smp2, per schedule call, exact-probe
  brackets):** pick segment 235 → 89–98 ns (−58%); exact GRQ hold
  (acquire→release) 378 → 241–273 ns (−28%); timeline op 46–47 ns
  measured, i.e. ~20–30 ns net of the probe's clock-read pair.
  Debug class: pick 602–608 → 444–452 ns.
- **The KVM tax is named, not hidden.** The lock-WAIT segment reads
  70–84 ns: the hypervisor emulates cache coherence in the management
  path, so cross-vCPU locked instructions cost several times more
  than on bare metal — the reference kernel's per-rq lock pays the
  same tax on this rig, so it is NOT a GRQ-specific cost. The site's
  old "~102 ns GRQ critical section" figure was this lock-WAIT
  segment mislabeled as the hold; the site's numbers now carry the
  full decomposition (lock wait / acct 110–122 / deq 58–64 / pick
  89–98 / ownership publish 68–77).
- **Honest floor:** the whole critical section holds at 241–273 ns
  because the remainder is D-1's own bookkeeping (class chain,
  ownership publish, set_curr, slice math, min_vruntime rebase,
  SWITCHING_OUT/slot/pick_gen stores, counter RMWs). A ≤10 ns
  whole-section class is unreachable without changing D-1 (ruled
  out); the tree-side component IS in the ~20–30 ns class and
  allocation-free.
- Gates: riscv64 make test 1333/1333 (5 new fair_timeline groups);
  triple-arch builds rc=0; pbench P2 median +5.6% release (inside
  ±20%) with p99 improved; census TERM 8 / REFUSED 32 in band; the
  fairness soak passes (F1 vruntime spread 5.8–11.2%, F2 wake→RT
  p99 ≈0.9–15 µs, F3 wait/run ≈3.0 on BOTH kernels = exact fair-share
  arithmetic for 4 hogs/CPU).
- Artifacts: run/GRQ-10NS.md (decomposition tables, poison-discipline
  notes, the honest-floor statement).

## S. Request merging + SG chains in the virtio-blk submit path (fio-merge-sg lane, 2026-11-18, evidence-backed)

- **The reference's blk-merge endpoint, typed.** Linux fuses
  queue-adjacent bios into one request carrying a scatter list
  (block/blk-merge.c `bio_attempt_back_merge`; virtio_blk maps it with
  `blk_rq_map_sg` into ONE descriptor chain — hdr + N data descs +
  resp). Rux now has the same shape at three layers, and the merge
  decision is a pure function of two integer compares
  (`sector_i + len_i == sector_{i+1}` — the back-merge test) over a
  recorded window: `WriteBatch` (bio) is a plug list whose flush()
  groups device-adjacent spans and submits one
  `blkdev_write_async_sg(disk, sector, &[DmaSeg], completion)` per
  group; `DmaSeg { ptr, len }` is the bio-segment contract made a
  `Copy` type — device-contiguous sectors, physically-discontiguous
  memory allowed; the ext4 direct-read window records runs and fuses
  at drain. Deferral to the drain is free by construction: chains
  publish QUIETLY and the device only sees work at the kick, so the
  plug list costs nothing the previous immediate submission did not.
- **The admission budget as a published-lifecycle counter.** The
  legacy in-flight guard assumes 3 descriptors per chain; an SG chain
  holds N+2, so chains-only accounting under-counts exactly when
  fusing is busiest. `VirtQueue::descs_held` is incremented at chain
  PUBLISH (pending store / sync tombstone, under the BLK lock) and
  released by the completion walker when it consumes the chain's
  used-ring entry — the same lifecycle the pending-table window
  reservation follows; burned (never-published) allocations never
  touch it. The window-overlap check
  (`chain_windows_overlap_len`) is length-aware and ring-wrap-safe.
  An SG chain remains ONE chain to the walker (one head, one ordinal,
  one pending, one used-ring entry matched by head id) — zero new
  completion machinery in the historically bug-dense ordering family.
- **Measured (q35 KVM smp8/8G, fio 3.36 psync depth 1, clean window
  load ~0.8, kernel-vs-kernel interleaved ×3, run/FIO-MERGE-SG.md
  §3.1):** 1 MiB sequential direct read 6857 → 8000 MiB/s (+17%),
  clat 139 → 118-120 µs (−21 µs/op); request count per op 4 → 1
  (diagnostic counters, since removed: "fused chains 1 (4 segs
  total), span-fallbacks 0" — every window one chain, zero
  fallbacks); 4k random read, 1 MiB write, 4k+fsync rows FLAT (±5%
  noise) — no regression band. fio --verify=md5 direct+buffered ×15
  total reps err=0, zero out-of-order/unmatched/misattribution
  classes in every serial.
- **Honest floor, named with numbers.** The fused window's whole wait
  phase (kick→completion→wake→return) is 131-134 µs, and a 1×1 MiB
  single-descriptor chain waits the same 131 µs as the 4-segment
  chain — the chain shape is FREE; the floor is kick→IRQ delivery
  (~60 µs class, fixed-CPU0 MSI steering — measured a conclusive dead
  end on this QEMU by submit-2x §3-C), the 1 MiB snapshot-overlay
  transfer, and the sleep/wake leg (~10-20 µs; D-21's adaptive
  poll-budget follow-up). The remaining +38 µs copyout term is the
  pinned-user-page DMA item (audit B2) whose plan builds on exactly
  these multi-segment chains. The audit's 0.9×-parity estimate for
  fusion alone was based on the 4 spans being 4 serialized
  kick→IRQ round trips; they shared one kick all along, so fusion
  buys the request-count term (+17%), not the delivery term.
- **ABI note that closes an audit extension:** virtio-blk requests
  address ONE contiguous sector range — the inode-table block cannot
  ride a data span's chain (not sector-adjacent). The wrfsync
  ride-along's shared kick window is the maximum batching the ABI
  allows; fsync4k measured flat with fusion, as that predicts.
- Gates: riscv64 make test green; triple-arch builds rc=0; pbench
  Tier-2 rows in band (p2_ctxsw 1.38 vs 1.24-1.27 band = +9%, within
  ±20%; p6_unlink 55.4 vs 54.6 flat; fs-row boards in
  run/pbench-fs.log); non-SG devices (MMIO virtio, loop) keep the
  byte-identical legacy submission shape (mode probed from the disk).
- Artifacts: run/FIO-MERGE-SG.md (A/B/C tables, floor decomposition,
  verification protocol), run/abc-*.txt, run/vfinal-*.log,
  run/pbench-gate.log, run/pbench-fs.log.
