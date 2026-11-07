# debug-lockdep-p1 lane record

Mission: lockdep-lite P1 — runtime lock acquisition tracking with
declared-order violation splats (docs/development/lockdep-lite-design.md §9
P1 list).

## User constraint (coordinator relay, 2026-11-07) — the HARD bar

Zero performance impact on the DEFAULT build:

- (a) DEFAULT build (cargo feature `debug-lockdep` OFF): ASM diff of
  KeyedSpinlock/KeyedRawSpinlock/SleepMutex acquire+release paths vs
  origin/main must be IDENTICAL; census A/B identical; perf-gate tiers
  1,2 in band on the default build. Merge-blocking leg.
- (b) Feature ON but `debug=lockdep` off (the armed-check class): the
  one-relaxed-load-per-site contract holds ONLY as the opt-in debug-boot
  configuration; measured and reported as opt-in ns/acquire.
- (c) Per-task held stack, BFS, splats, /proc live view: all inside the
  feature, all opt-in.

Enforcement in code: every hook call site is `#[cfg(feature =
"debug-lockdep")]` (the OFF build does not even contain the calls), the
`lockdep_held` Task field, the NestKey class const, the SleepMutex class
field, and the whole runtime module are feature-gated. Two codegen traps
were caught and fixed by the ASM gate itself (see below).

## What landed (design §9 P1 list)

1. **Cargo feature `debug-lockdep`** (kernel/Cargo.toml) — the compile
   gate. OFF = hooks do not exist (see ASM identity). The existing
   `debug=lockdep` boot switch arms runtime tracking in addition to the
   P0 boot validation (`boot_validate()` calls `arm_runtime()` +
   `boot_selftest()` under the feature).
2. **Hooks, keyed lock types (design §5 surface 1)** — the key IS the
   class, zero call-site edits outside sync/: acquire hooks in
   `KeyedRawSpinlock::{lock, try_lock, try_lock_claim, lock_claim}`
   (+`lock_nested` via `lock`), release hooks in `unlock` + the
   `KeyedGuard`/`KeyedIrqGuard` drops, acquire in
   `KeyedSpinlock::lock_irqsave`. Each key type now carries
   `NestKey::LOCK_CLASS` (feature-gated const referencing the registry
   id consts; the armed path cross-checks `CLASSES[id].name == NAME` so
   drift is loud).
3. **Hooks, the SleepMutex pair (design §5 surface 2)** —
   `SleepMutex::with_lockdep_class()`; VFS_MUTATION_LOCK and
   EXT4_BIG_LOCK carry their registry classes. Acquire hook at the
   `lock()` funnel entry (the INV-LOCK-8 context check must fire before
   any parking), release hook in `release()` and `release_if_owner()`.
4. **Checks (design §6)**, in order: same-class task-recursive re-entry
   is legal; SLEEP-IN-ATOMIC-CONTEXT (Sleep domain + any spin-family
   held OR preempt_count != 0 — INV-LOCK-8 rule 1); ORDER-VIOLATION
   (acquiring a class that a declared edge names OUTER while an INNER
   is held — FWD-bitset lookup, invariant id named); chain-key
   amortization (64-bit FNV-1a over the held stack + new id, lock-free
   open-addressing table, hit = no lock taken at all); new-chain path
   under one leaf graph spinlock: edge insert (with acquire site) +
   UNDECLARED-NEST note (once per pair, at insert; not a splat) +
   bounded BFS (queue 32) over observed+declared edges for a cycle →
   CYCLE splat with the reconstructed path. Splats name both classes,
   the invariant id, ksyms-symbolized acquire/hold sites, and the
   task/pid/cpu/preempt/in_irq context; first splat self-disables the
   checker and sets TAINT_WARN (reference `debug_locks=0` semantics);
   arena exhaustion (chains/edges/depth/BFS bound) disables too.
5. **/proc/lockdep live view** (declared FD/BD + observed edges with
   symbolized sites + counter tail: acquires/chain-hits/chains/edges/
   bfs/undeclared-notes/splats + armed/disabled state) and
   **/proc/lockdep_chains** (`irq_context:` + `[class] name` rows).
   Divergence (documented in-code): every chain's 64-bit key is kept in
   the amortization table, but the class LIST is retained for the most
   recent 128 chains (bounded ring); the reference stores every chain's
   content.
6. **Synthetic proof** — `boot_selftest()` runs under the same
   `debug=lockdep` arm at boot: s0 key<->registry mapping, cost probe
   (see measurement), s1 declared forward passes, s2 the FORCED
   reverse of INV-LOCK-5 (GRQ held, FUTEX_BUCKET acquired) → the money
   splat, s3/s4 the INV-LOCK-8 negatives (spin-held arm and preempt
   arm), s5 a synthetic two-task observed cycle. State resets after;
   runtime tracking starts clean. ktests under
   `unit-test,debug-lockdep` cover the same verdicts + chain-key
   amortization counters + the /proc renders; the whole unit-test suite
   runs with the checker ARMED (design §10).

### Scope note (honest)

GRQ's lock is a plain `RawSpinlock` on main (the KeyedRawSpinlock
adoption was withdrawn after 2b092c49; re-adoption staged), and
NetQueueKey/NetStatsKey have no live instances either. So the LIVE
hooked surfaces today are FUTEX_BUCKET + WAITER_SLOT (the futex
funnel: every futex syscall) and VFS_MUTATION/EXT4_BIG. The GRQ/NET
classes and their hooks exist and are proven by the synthetic tests;
runtime observation of those classes lands with the staged keyed
re-adoption (one-line const per key already in place).

### Codegen traps the ASM gate caught (kept for the record)

1. An introduced `let g = self.inner.lock_irqsave();` binding in
   `KeyedSpinlock::lock_irqsave` shifted O0 codegen even with the hook
   cfg'd out (extra spill + int3 padding). Fix: cfg-split method bodies
   — the OFF arm keeps the original expression shape verbatim.
2. `#[inline]` hook wrappers in SleepMutex emit REAL empty calls at
   opt-level 0 (dev profile honors inline late). Fix: the hook fns and
   their call sites are feature-only.

## Results

### DEFAULT-build ASM identity (the merge-blocking hard bar) — PASS

`run/asm_identity_diff.sh <main-elf> <branch-elf> <arch>` (in-tree;
per-symbol disassembly, addresses normalized, instruction stream +
symbolic targets compared):

- x86_64 dev build, origin/main@97189069 vs branch (feature off):
  **447/447 lock-path symbols IDENTICAL** (keyed lock types' every
  method, RawSpinlock lock/unlock/try_lock, SleepMutex
  lock/release/lock_inner/release_if_owner/optimistic_probe, guard
  drop glue).
- riscv64 dev build, same pair: **443/443 IDENTICAL**.

### riscv64 make test

- feature ON + checker ARMED for the whole suite
  (`riscv64,unit-test,debug-lockdep`, unittest-boot-riscv.py):
  **ALL TESTS PASSED** (1360 PASS lines), 11/11 p1 runtime cases
  green; the only LOCKDEP serial lines are the test group's own
  deliberate scenarios — zero splats/notes from the suite's real
  futex/fs/sched traffic (design §10's silence requirement).
- feature OFF (`make test` standard battery): **ALL TESTS PASSED**
  (1343 PASS lines — the suite minus the 17 feature-gated lockdep-P1
  additions, which is exactly the count delta to the armed run).

### x86_64+KVM armed proof boot (the money output)

pbench boot with `debug=lockdep` on the feature-ON bzImage
(work/pbench-lockdep-armed-serial.log, PBENCH DONE rc=0):

```
lockdep: selftest s0 key<->registry mapping: PASS
lockdep: cost-probe keyed acquire/release pair (N=200000): disarmed 38 ns, armed 129 ns (hook delta 90 ns)
lockdep: selftest s1 declared forward (bucket -> grq): PASS
LOCKDEP: ORDER-VIOLATION grq -> futex-bucket contradicts INV-LOCK-5 (declared: futex-bucket -> grq)
  acquiring: futex-bucket at rux::debug::lockdep::runtime::arm_runtime+...
  held:      grq at ... (depth 0)
  task= pid=0 cpu=0 preempt=0x0 in_irq=false in_softirq=false
LOCKDEP: disabling lockdep (first splat; debug_locks=0 equivalent)
lockdep: selftest s2 declared-order violation (grq held, bucket acquired): PASS (splat above)
LOCKDEP: SLEEP-IN-ATOMIC-CONTEXT acquiring sleep-domain vfs-mutation (INV-LOCK-8 rule 1)   [s3 PASS]
LOCKDEP: SLEEP-IN-ATOMIC-CONTEXT acquiring sleep-domain ext4-big (INV-LOCK-8 rule 1)       [s4 PASS]
LOCKDEP: CYCLE inode -> process-tree -> inode                                             [s5 PASS]
lockdep: selftest PASS (s0 mapping + s1-s5 scenarios)
```

Zero LOCKDEP lines from the real pbench workload (futex storm, fs
writes through VFS_MUTATION/EXT4_BIG, 100k context switches) — the
checker ran silent, then the whole P1-P8 battery completed.

### Measurement (design §7 rows)

| row | verdict |
|---|---|
| 1 feature-OFF build identity | **PASS** — ASM identical both arches (above); dual-arch builds rc=0 |
| 2 feature-OFF tier 1 syscost | [pending] |
| 3 feature-ON disarmed tier 2 (pbench) | **PASS** — every anchor row within band or improved: p1_null 0.281 (anchor 0.300), p1_clock 0.361 (0.371), p2_ctxsw 1.232 (1.242), p3b_cow 9.377 (8.075, +16% < 20%), p6_write_fsync 8391 (8443), p6_mkdrmdir 224.1 (216.7), p6_unlink 57.7 (54.6), p6_append 5.41 (5.19); disarmed boot emitted zero LOCKDEP lines |
| 4 feature-ON armed (report class) | armed boot medians: p1_null 0.287, p2 1.29-ish class, p8 rows below; +110 ns/futex-wake vs disarmed — the armed per-acquire class |
| 5 pbench P8 lock-pair row | table below |

**P8 lock-pair rows** (median us; same KVM boot class, quiet-ish
window at host load ~6-7):

| build | p8_futex_wake (uncontended keyed bucket pair) | p8_futex_pp (two-task ping-pong half-rt) |
|---|---|---|
| ON, disarmed (one relaxed load/site) | 0.842 | 4.704 |
| ON, armed (full pipeline, chain-hit) | 0.952 | 5.150 |
| armed delta | +110 ns | +446 ns |
| OFF (default build) | from the perf-gate tier-2 boot | [pending] |

In-kernel cost probe (x86_64/KVM, N=200k pairs, O0 debug build):
disarmed 38 ns/pair (~19 ns per acquire site: relaxed load + ra
capture + branch), armed 129 ns/pair steady-state (chain-key hit
path). These are the opt-in numbers the coordinator's constraint (b)
refers to; the DEFAULT build carries none of it (constraint (a),
ASM-identical).

The checker-internal counters (design §7 row 4: chains/BFS/edges) are
exercised by the ktest assertions (amortization counters + the
/proc/lockdep tail renders them live).

### Reference-alignment round (user directive 2026-11-07)

- Splats reworked to the reference lockdep report shape (banner,
  acquiring/held pair with symbolized sites + raw ips, reverse
  dependency chain, CPU0/CPU1 diagram ending *** DEADLOCK ***, held
  list; INV-LOCK-8 case as "sleeping lock in atomic context"). The
  self-disable line uses the reference wording ("turning off the
  locking correctness validator, debug_locks = 0"); the semantic was
  already in (first splat stops all checking).
- D-21 registered in docs/architecture/design-divergences.md (the
  Rust-native mapping differences vs the reference lockdep).
- DEFAULT-build ASM identity RE-VERIFIED after these changes:
  x86_64 447/447, riscv64 443/443 (splat code is feature-gated).
- Official dual-arch build commands: `PLATFORM=x86_64 make build` rc=0;
  `rm build/.config && make build` (riscv64) rc=0.

## Boots/artifacts

- worktree: /home/william/rux-agents/debug-lockdep-p1 (branch
  debug-lockdep-p1 from origin/main@97189069)
- baseline worktree for ASM A/B: /home/william/rux-agents/debug-lockdep-p1-base
- feature-on x86_64 bzImage: target/x86_64-unknown-none/debug/bzImage-lockdep
- default x86_64 bzImage: target/x86_64-unknown-none/debug/bzImage
