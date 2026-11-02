# arch-s1 lane: S-1 model-checking pilot (Kani/loom on kernel concurrency primitives)

## Charter (from the survey shortlist, docs/architecture/os-kernel-survey.md S-1; user go 2026-11-03)
Pilot, 1-2 sessions: evaluate whether Kani (verification) and loom
(concurrency model checking) can run against Rux kernel concurrency
primitives in a HOST-side harness, and whether they pay. This is the
methodology adoption the survey recommends (seL4-lineage verification
methods applied to Rust-native primitives).

## Known constraint going in
The kernel crate cannot link std (no_std; unit tests collide with
panic_impl — recorded house lesson). loom/kani need host builds. The
pilot's core question is EXTRACTION cost: can the primitive's logic be
built for the host (cfg-gated, or extracted to a small inner crate)
without semantic drift?

## Targets (in order)
1. kernel/src/sync/mutex.rs SleepMutex (the R84 handoff state machine:
   waiters/tokens/anti-starve — historically bug-dense, now the
   canonical primitive).
2. kernel/src/sync/spinlock.rs + the preempt-count interplay (if time).
3. The futex wait-queue key resolution (physical-word keys from the
   LTP fix) if S-1 finishes early.

## Deliverables
- A host-side harness (kani/loom configs in-repo, English comments)
  with the extraction pattern documented.
- Verdict per target: invariants checked, bugs found (loom explores
  interleavings; kani proves bounds/overflow), false-positive load,
  runtime cost, and whether the extraction changed the code shape.
- A do/don't recommendation for CI integration (GitHub smoke.yml runs
  kani on nightly already for some crates — check .github/workflows).
- NO kernel-behavior changes: extraction shims must be cfg(test)-like
  and default-off; if a real bug is FOUND, fix it through the normal
  lane discipline (separate commit, gates).

## Session log (append per session)
