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

- feature OFF (default `make test` battery): see below [pending]
- feature ON + checker ARMED for the whole suite
  (`riscv64,unit-test,debug-lockdep`): see below [pending]

### Measurement (design §7 rows)

| row | status |
|---|---|
| 1 feature-OFF build identity | PASS (above) + dual-arch builds rc=0 |
| 2 feature-OFF tier 1 syscost | [pending] |
| 3 feature-ON disarmed tier 1+2 | [pending] |
| 4 feature-ON armed (report class) | [pending] |
| 5 pbench P8 lock-pair row (OFF/disarmed/armed) | [pending] |

The in-kernel cost probe (debug=lockdep boot) prints the direct
disarmed-vs-armed ns per keyed acquire/release pair; the pbench P8 rows
(`p8_futex_wake` uncontended keyed bucket pair, `p8_futex_pp` two-task
futex ping-pong) measure the same surface through the syscall funnel
for OFF vs ON-disarmed vs ON-armed builds.

## Boots/artifacts

- worktree: /home/william/rux-agents/debug-lockdep-p1 (branch
  debug-lockdep-p1 from origin/main@97189069)
- baseline worktree for ASM A/B: /home/william/rux-agents/debug-lockdep-p1-base
- feature-on x86_64 bzImage: target/x86_64-unknown-none/debug/bzImage-lockdep
- default x86_64 bzImage: target/x86_64-unknown-none/debug/bzImage
