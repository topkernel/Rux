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

pbench boot with `debug=lockdep` on the feature-ON bzImage built at
final HEAD (work/pbench-lockdep-armed2-serial.log, PBENCH DONE rc=0 at
t=69s; the earlier armed log pbench-lockdep-armed-serial.log predates
the reference-shape rework 61f65804 and is superseded):

```
lockdep: selftest s0 key<->registry mapping: PASS
lockdep: cost-probe keyed acquire/release pair (N=200000): disarmed 38 ns, armed 128 ns (hook delta 90 ns)
lockdep: selftest s1 declared forward (bucket -> grq): PASS
=========================================================
WARNING: possible circular locking dependency detected            <- reference banner
---------------------------------------------------------
?/0 on cpu 0 is trying to acquire lock:                          <- task/cpu context line
 (futex-bucket){....}, at: rux::debug::lockdep::runtime::arm_runtime+0x4/0x4c (ip 0xffffffff8040e664)
but task is already holding lock:
 (grq){....}, at: rux::debug::lockdep::runtime::arm_runtime+0x0/0x4c (ip 0xffffffff8040e660)

the existing dependency chain (in reverse order) is:             <- reference -> #N chain
-> #1 (grq){....}  [declared INNER, INV-LOCK-5]
-> #0 (futex-bucket){....}  [declared OUTER, INV-LOCK-5]

 ORDER-VIOLATION of INV-LOCK-5: the declared order is futex-bucket -> grq (docs/architecture/lock-ordering.md); observed the reverse
                                                                  <- Rux addition: the INV-LOCK-* citation
 Possible unsafe locking scenario:
       CPU0                    CPU1
       ----                    ----
  lock(futex-bucket);
                               lock(grq);
                               lock(futex-bucket);
  lock(grq);
 *** DEADLOCK ***                                                 <- reference closing line

1 lock(s) held by this task:                                     <- reference numbered held list
 #0: (grq){....}, at: rux::debug::lockdep::runtime::arm_runtime+0x0/0x4c
cpu=0 preempt_count=0x0 in_hardirq=false in_softirq=false        <- Rux addition: context tail
LOCKDEP: turning off the locking correctness validator (debug_locks = 0; first splat)
                                                                  <- reference self-disable wording
lockdep: selftest s2 declared-order violation (grq held, bucket acquired): PASS (splat above)
```

The other selftest scenarios in the same shape: s3/s4 print
`WARNING: sleeping lock in atomic context (INV-LOCK-8 rule 1)
detected` with the held spin lock (grq -> vfs-mutation) or the
preempt_count arm (ext4-big, preempt_count=0x1) named; s5 prints the
CYCLE splat with the observed-edge chain (inode <-> process-tree,
INV-LOCK-4) before the same CPU0/CPU1 diagram. All five PASS; usage
chars print `{....}` until P2 arms irq-state tracking.

Zero LOCKDEP lines from the real pbench workload (futex storm, fs
writes through VFS_MUTATION/EXT4_BIG, 100k context switches) — the 10
splat/warning lines in the whole serial are all the selftest's own;
the checker ran silent, then the whole P1-P8 battery completed.

### Measurement (design §7 rows)

| row | verdict |
|---|---|
| 1 feature-OFF build identity | **PASS** — ASM identical both arches (above); dual-arch builds rc=0, re-verified at final HEAD (x86_64 default rebuild sha256-identical to the gated image; riscv64 `rm build/.config && make build` rc=0) |
| 2 feature-OFF tier 1 syscost | **PASS — scripted verdict, zero FAIL** (run/perf-gate-lockdep-p1-default-v3-verdict.txt; 780s quiet boot run/pa-lockdep-p1-default-stamped.log, 22:53–23:07 window, foreign_qemus<=2): panics 0 / OOM 0; census TERM 8 (8±2) / REFUSED 32 (32±4), no 1001ms grid class (STRANDED 9 organic); ticks 122.07/121.08 /CPU (min 99.5); syscall volume 135,520,840 (126M ±10%); clock_gettime 3us (3–5); nanosleep 122.9ms (<=244); GRQ hold 4732ns (1200–7320); small_alloc REST share 0.5% (<=10%); buffer-cache/fsw SKIP (instruments not in this build). Third-launch note: the verdict pass originally SKIPped volume because grep read the ANSI/NUL-bearing stamped log as binary; re-run on the cleaned copy run/pa-lockdep-p1-default-stamped-clean.log — every metric now carries a real verdict, all PASS. |
| 3 feature-ON disarmed tier 2 (pbench) | **PASS** — every anchor row within band or improved: p1_null 0.281 (anchor 0.300), p1_clock 0.361 (0.371), p2_ctxsw 1.232 (1.242), p3b_cow 9.377 (8.075, +16% < 20%), p6_write_fsync 8391 (8443), p6_mkdrmdir 224.1 (216.7), p6_unlink 57.7 (54.6), p6_append 5.41 (5.19); disarmed boot emitted zero LOCKDEP lines |
| 4 feature-ON armed (report class) | armed boot medians: p1_null 0.287, p2 1.29-ish class, p8 rows below; +110 ns/futex-wake vs disarmed — the armed per-acquire class. Re-measured at final HEAD (armed2 boot): p1_null 0.281, p2 1.312, p8_futex_wake 0.922, p8_futex_pp 5.160 — same class |
| 5 pbench P8 lock-pair row | table below |

**P8 lock-pair rows** (median us; same KVM boot class, quiet-ish
window at host load ~6-7):

| build | p8_futex_wake (uncontended keyed bucket pair) | p8_futex_pp (two-task ping-pong half-rt) |
|---|---|---|
| ON, disarmed (one relaxed load/site) | 0.842 | 4.704 |
| ON, armed (full pipeline, chain-hit) | 0.952 | 5.150 |
| armed delta | +110 ns | +446 ns |
| OFF (default build, no hooks compiled) | 0.862 | 4.388 |

The OFF row comes from the default-build tier-2 pbench boot
(run/perf-gate-lockdep-p1-default-pb.log, foreign_qemus=2). OFF sits
within noise of ON-disarmed on the wake row (0.862 vs 0.842, both in
the 0.84-0.86 us class for this window) — consistent with the
disarmed contract being one relaxed load per site; ON-armed carries
the +110 ns pipeline cost. The pp row is two-task scheduler-bound and
window-sensitive (4.4-5.2 across same-class boots).

### Feature-OFF tier-2 anchor compare (third launch)

Scripted anchor compare (the perf-gate.sh tier-2 python,
run/tier2-anchor-compare.py) on the default-build serial
work/pbench-lockdep-p1-default-pb-serial.log
(run/perf-gate-lockdep-p1-default-tier2-anchor.txt):
p1_null 0.271/0.300, p1_clock 0.351/0.371, p2_ctxsw 1.247/1.242,
p6_unlink 60.3/54.6, p6_write_fsync 9210.5/8442.6 all PASS; but
**p6_append 6.422 (anchor 5.189, +24%) and p6_mkdrmdir 271.9 (anchor
216.7, +25%) above the +20% band** — with mkdrmdir p99 ~38s / max
~62s: host-stall signature, and the boot started one minute after the
780s syscost boot ended (23:07). The same rows on the feature-ON
DISARMED boot in the banked row-3 window were in band (5.41, 224.1),
and the default build is lock-path ASM-identical to main, so the
rerun-once rule applies: isolated rerun in a quiet window (detached
wait-and-retry run/tier2-retry.sh, polls for <=2 foreign QEMUs).
Verdict appended below in the third-launch section when it lands.

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
  Fourth-launch rebase note: main's dlsleep merge (87ed94dc) landed its
  own D-21/D-22 (hung-task owner chain, panic last-gasp); the lane entry
  is renumbered **D-23** on the rebased branch.
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
- tier-1 verdict (third launch): run/perf-gate-lockdep-p1-default-v3-verdict.txt
  (cleaned log source: run/pa-lockdep-p1-default-stamped-clean.log)
- tier-2 anchor compare (default build, 23:07 boot):
  run/perf-gate-lockdep-p1-default-tier2-anchor.txt
- tier-2 quiet-window rerun (wait-and-retry): run/tier2-retry.sh,
  output run/tier2-retry-out.log, serial
  work/pbench-lockdep-p1-default-v3-pb-serial.log when it fires
- fourth launch (post-rebase): armed boot serial
  work/pbench-lockdep-armed3-serial.log, suite logs
  run/unittest-featureon-rebase.log / run/maketest-off-rebase.log,
  regression A/B logs run/riscv-regression-boot-rebase2.log +
  run/riscv-regression-boot-MAIN2.log, preserved default ELFs
  run/x86-default-final-rux + run/riscv-default-final-rux, gate loop
  run/tier12-retry-rebase.sh → run/tier12-retry-rebase-out.log,
  gate serials run/pa-lockdep-p1-rebase-serial.log +
  work/pbench-lockdep-p1-rebase-pb-serial.log

## Resume state (11-08, third launch)

### What the second launch finished (uncommitted until now)

Tier-1 verdict computed and row 2 filled; feature-ON rebuilt at HEAD
(20:20) + armed2 KVM proof boot captured (20:21,
work/pbench-lockdep-armed2-serial.log — the money excerpt above is
that boot's output, verified verbatim); OFF P8 cells filled from the
default-build pbench boot. Third launch verified all of it against the
artifacts on disk before committing (below).

### Third-launch verification (all against on-disk artifacts)

1. **Dirty-file reconciliation**: the modified run/LANE.md (the second
   launch's fill — kept, re-checked) and the untracked
   `toolchain/x86_64-rux-linux-musl` (a symlink to the debug-lockdep
   lane's toolchain dir, needed for x86_64 builds) — the symlink is
   now .gitignored next to the riscv64 toolchain entry; nothing else
   was dirty, no code moved since 61f65804 (kernel diffs of the last
   two commits: docs-only + feature-gated-only).
2. **Tier-1 verdict (the leg the predecessor died on)**: re-run
   scripted on an ANSI/NUL-cleaned copy of the stamped log — ALL PASS,
   zero FAIL (run/perf-gate-lockdep-p1-default-v3-verdict.txt; row 2
   above updated: volume now a real PASS at 135.5M, not SKIP).
3. **ASM identity re-verified live at HEAD** with the on-disk pair
   (base worktree main@97189069 ELFs vs this worktree's Nov-8 20:20/20:24
   default-build ELFs): x86_64 **447/447 IDENTICAL**, riscv64
   **443/443 IDENTICAL** (run/asm_identity_diff.sh re-run, third
   launch). Valid without rebuild: code has not moved since 61f65804
   and the OFF build is provably untouched by the splat rework (all
   its hunks are inside the feature-gated `mod runtime`).
4. **make test evidence**: ON+armed suite re-checked
   (run/unittest-featureon-final.log, 22:36 Nov 7 = post-rework code:
   "All tests passed", 1360 PASS lines, all 11 p1 runtime cases PASS —
   including "reverse of INV-LOCK-5 splats" (violation) and
   "sleep-under-spin" + "sleep-in-preempt-ctx" (the context
   negatives)); OFF suite green stands for HEAD because the rework
   touched only feature-gated code (1343 PASS, recorded in the
   committed round).
5. **riscv64 regression boot**: run/riscv-regression-boot2.log —
   /proc loop + churn completed (LOOP-DONE, loop_done=True), 0
   KERNPANIC lines; the driver's `wedge_reported: True` is the BENIGN
   branch ("loop finished, then quiet (shell exited?)") — checked in
   the driver source (scripts/test/boot-soak-riscv.py) and log.
6. **Tier-2 on the DEFAULT build**: anchor compare run — 5/7 anchored
   rows PASS, p6_append/p6_mkdrmdir above band (see the anchor-compare
   section above; host-stall tails, rerun-once rule). Detached
   wait-and-retry launched (run/tier2-retry.sh, log
   run/tier2-retry-out.log): polls every 5 min for <=2 foreign QEMUs,
   then `perf-gate.sh --tiers 2` on
   target/x86_64-unknown-none/debug/bzImage (tag
   lockdep-p1-default-v3, serial
   work/pbench-lockdep-p1-default-v3-pb-serial.log).

### Remaining for the successor (if quota death before the rerun lands)

1. Poll `run/tier2-retry-out.log`. If the rerun completed: paste its
   anchor-compare verdict into the anchor-compare section above; if
   the two fs rows are in band (or improved), the battery is green.
   If still above band in a genuinely quiet window, that is a real
   lead — but note the same rows are in band on the feature-ON
   disarmed boot and the default build is ASM-identical to main, so
   weigh against host noise first (the gate's own note).
2. Commit this record (plus .gitignore) if not yet committed; PUSH
   origin debug-lockdep-p1 only when the battery is green.
3. If no quiet window in ~5h (60 attempts), keep the retry verdict as
   documented-pending and let the coordinator decide (the retry loop
   exits 4).

Host note at launch: 8 foreign QEMUs, load ~9.6 — the gate's own
preflight would self-refuse; hence wait-and-retry instead of stacking
a boot.

## Fourth launch (11-10): rebase onto main@ccac40ee + full re-verification

### Rebase

origin/main moved 122 commits past the 97189069 base. `git rebase
origin/main`: ONE conflict — docs/architecture/design-divergences.md,
where main's dlsleep merge (87ed94dc) added its own D-21 (hung-task
owner chain) and D-22 (panic last-gasp) while the lane commit added a
D-21; resolved by keeping both and renumbering the lane entry to
**D-23** (next free number; the in-code and LANE references updated).
Semantic reconciliation of the warned areas:

- `sync/mutex.rs` (dlsleep merge): the merge's changes are the
  blocked_on store in the contended wait path and owner-RA diagnostics;
  the P1 hooks sit at the `lock()` funnel entry and the
  `release()`/`release_if_owner()` funnels — both sides merged clean
  and the pairing (one push per lock(), one pop per release path
  including the exit force-drop) is unchanged.
- `sync/spinlock.rs` (main's fc718f82/08c99b83 owner-RA commits): the
  `caller_return_address` doc changed on main; the lane's
  `pub(crate)` promotion (the P1 acquire-site source) merged with it.
  Codegen unchanged (visibility only).
- futex (main's c7f0fe48 private-key hashing): the bucket is still the
  `KeyedSpinlock<FutexBucketKey>` funnel; key hashing changed, lock
  type did not — hooks intact.

### Re-verification on the rebased tree (all fresh, this launch)

1. **Builds x3, rc=0**: `PLATFORM=x86_64 make build` (default);
   `rm build/.config && make build` (riscv64 default); feature-ON
   x86_64 (cargo `x86_64,debug-lockdep` + ksyms two-pass +
   mkbzimage → bzImage-lockdep).
2. **DEFAULT-build ASM identity vs CURRENT origin/main**
   (base worktree rebuilt at ccac40ee, same-day pair):
   **x86_64 451/451 IDENTICAL, riscv64 447/447 IDENTICAL.** Counts
   grew from 447/443 — main added lock-path symbols under the same
   filter, all identical. One riscv pair initially differed:
   `SpinlockIrqGuard<TrimQEntry>::drop -> RawSpinlock::unlock` split
   its PC-relative target differently between the auipc immediate and
   the jalr offset (4KiB page-boundary; whole-binary layout artifact
   from the rebase, not a codegen choice — same resolved symbol).
   The gate's normalizer now folds auipc+consumer pairs to the
   effective symbolic target (commit 5c3b3876; strict on resolved
   targets — a different symbol still fails). Post-fix: both arches
   fully identical.
3. **Feature-ON armed suite (riscv64, unit-test+debug-lockdep,
   checker ARMED for the whole suite)**: **ALL TESTS PASSED, 1360
   PASS lines, 11/11 p1 runtime cases green** — including "reverse of
   INV-LOCK-5 splats", "sleep-under-spin", "sleep-in-preempt-ctx",
   "observed cycle splats", chain-key amortization, both /proc
   renders (log run/unittest-featureon-rebase.log).
4. **Feature-OFF suite (`make test`, riscv64)**: **rc=0, ALL TESTS
   PASSED, 1349 PASS lines** (log run/maketest-off-rebase.log) —
   exactly the armed count minus the 11 feature-gated p1 cases.
5. **riscv64 regression boot (default build, smp8 TCG, 150s)**: rc=0,
   zero KERNPANIC/panics/POISONED, LOOP-DONE printed
   (run/riscv-regression-boot-rebase2.log). The window carried
   TIMER-SOFTIRQ-STALL/WAKE-STRANDED markers (21/10/2) under host
   load ~3.5 with 4+ foreign QEMUs; the same-rig A/B on PURE
   origin/main in the same window shows the same class (13/6/2,
   run/riscv-regression-boot-MAIN2.log) — TCG-under-load starvation,
   not a branch regression (the third launch's zero-marker boot ran
   in a quieter window). Merge-gate criteria (rc=0, zero panics)
   hold.
6. **Armed ns/acquire RE-MEASURED post-rebase** (constraint (b);
   bzImage-lockdep, KVM, debug=lockdep, PBENCH DONE rc=0,
   work/pbench-lockdep-armed3-serial.log): cost-probe keyed
   acquire/release pair (N=200000): **disarmed 37 ns, armed 128 ns
   (hook delta 91 ns)** — same class as the banked 38/128/90 (hooks
   did not move materially). All five selftest scenarios PASS with
   splats; the only WARNING/LOCKDEP lines in the whole serial are
   the selftest's own 10 plus one pre-existing CR4.SMAP arch warning
   (unrelated, image class) — zero splats/notes from the real pbench
   traffic (futex storm, fs writes, 100k+ context switches).
   Armed pbench rows this boot: p1_null 0.280, p2_ctxsw 0.771,
   p8_futex_wake 0.952 (banked armed 0.952 — identical),
   p8_futex_pp 10.269 (the pp row is two-task scheduler-bound and
   window-hostage: this boot ran at foreign_qemus=4 with p2 p99
   65 ms / max 510 ms spikes; quiet-window class 4.4-5.2 stands).

### Annotated violation-splat excerpt (usage guide, fresh post-rebase boot)

From work/pbench-lockdep-armed3-serial.log — the forced reverse of
INV-LOCK-5 (selftest s2; the shape is what any real ORDER-VIOLATION
will print). Annotations are in the right column; the DEFERFREE line
is an unrelated serial interleave from another subsystem on the same
console.

```
WARNING: possible circular locking dependency detected            <- reference banner
---------------------------------------------------------
?/0 on cpu 0 is trying to acquire lock:                          <- task/tgid/cpu context
 (futex-bucket){....}, at: rux::debug::lockdep::runtime::successors+0x134/0x136 (ip 0xffffffff8042b5f4)
                                                                 ^ acquiring class + ksyms-symbolized site + raw ip
but task is already holding lock:
 (grq){....}, at: rux::debug::lockdep::runtime::successors+0x130/0x136 (ip 0xffffffff8042b5f0)
                                                                 ^ held class + its acquire site

the existing dependency chain (in reverse order) is:             <- reference -> #N chain
-> #1 (grq){....}  [declared INNER, INV-LOCK-5]
-> #0 (futex-bucket){....}  [declared OUTER, INV-LOCK-5]

 ORDER-VIOLATION of INV-LOCK-5: the declared order is futex-bucket -> grq (docs/architecture/lock-ordering.md); observed the reverse
                                                                 ^ Rux addition: the INV-LOCK-* citation (what to fix and where)
 Possible unsafe locking scenario:
       CPU0                    CPU1
       ----                    ----
  lock(futex-bucket);
                               lock(grq);
                               lock(futex-bucket);
  lock(grq);
 *** DEADLOCK ***                                                 <- reference closing line

1 lock(s) held by this task:                                     <- reference numbered held list
 #0: (grq){....}, at: rux::debug::lockdep::runtime::successors+0x130/0x136
cpu=0 preempt_count=0x0 in_hardirq=false in_softirq=false        <- Rux addition: context tail
LOCKDEP: turning off the locking correctness validator (debug_locks = 0; first splat)
                                                                 ^ reference self-disable semantics: first splat stops all checking
lockdep: selftest s2 declared-order violation (grq held, bucket acquired): PASS (splat above)
```

Reading guide for a REAL splat (not the selftest): the "at:" site on
the ACQUIRING line is where the bad order was observed (ra captured
at the hook); the held-list "at:" is where the INNER lock was taken;
the ORDER-VIOLATION line names the declared forward order and the
lock-ordering.md invariant to fix against. The other splat families:
"WARNING: sleeping lock in atomic context (INV-LOCK-8 rule 1)
detected" names the held spin class or the preempt_count arm; the
CYCLE splat prints the observed-edge chain before the same
CPU0/CPU1 diagram. UNDECLARED-NEST lines are NOTES (once per pair),
not splats — they feed lock-ordering.md, checking continues.

### Merge-gate tiers on the DEFAULT build (the missing leg)

Third-launch context: tier 1 green (row 2 above); tier 2 had two fs
rows above the OLD anchor band in a host-stalled window (23:07 boot,
one minute after the 780s syscost boot ended). NOTE: perf-gate.sh's
tier-2 anchor was RE-BANKED on main 2026-11-08 for the SMP-8
standard (p6_append 6.402, p6_mkdrmdir 269.778, p2_ctxsw 0.756,
p1_null 0.280, p1_clock 0.350, p3b_cow 18.424, p6_write_fsync
8613.105, p6_unlink 67.735) — against the re-banked anchor, the
third launch's measured rows (append 6.422, mkdrmdir 271.9) are IN
band (+0.3%, +0.8%). The re-banked anchor ships in the perf-gate.sh
this lane runs, so the fourth-launch tier-2 run is judged against it
directly.

Fourth launch: wait-and-retry loop (run/tier12-retry-rebase.sh,
detached; fires `perf-gate.sh --tiers 1,2 --kernel
target/x86_64-unknown-none/debug/bzImage --tag lockdep-p1-rebase
--secs 780` at <=1 foreign QEMU). Verdict below when it lands.
