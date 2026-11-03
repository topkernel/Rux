# The Rust-Native Register — designs Rux ships that a translation could not

Purpose (user directive 2026-11-03): every architectural advantage —
performance, safety, correctness, observability — is RECORDED HERE with
evidence, so a reader sees original engineering, not a C-to-Rust
transcription. The companion register for interface-level divergences is
docs/architecture/design-divergences.md (D-numbers); this file carries
the narrative, the evidence, and the "why a translation would not have
produced this" argument. RULE: an entry without measured evidence is a
placeholder, not an entry.

## How to read the register
- Every entry passed the three-gate rule (external POSIX/ABI compat
  intact + LTP passes the affected families + no upper-layer software
  breaks) — divergence is earned, not asserted.
- "Translation would not have produced this" means: the design depends
  on Rust's type system, ownership model, or safe-construction
  guarantees such that a line-by-line port of the reference kernel
  structurally cannot arrive at it.

## A. Type-system-enforced designs
- **D-19 Typed timer wheel** (kernel/src/timer/): one-shot vs periodic,
  IRQ-context vs task-context callback classes, armed/pending/fired
  typestate — timer misuse that mainstream kernels debug at runtime is
  a COMPILE ERROR here. Evidence: six rustc type-catch classes
  (run-type-catches.txt); timer-lock holds -25..33% per op (arm
  924→697 ns, pending 1288→868 ns, del 1291→862 ns, matched volumes);
  p2_ctxsw 15.4→12.2 µs.
- **S-1 Model-checked concurrency primitives**: loom interleaving
  suites + Kani proofs against the SleepMutex handoff state machine
  (mirror-crate extraction, zero kernel restructuring). Evidence: loom
  8 scenarios 59.5 s catching 3 planted bugs incl. a pre-fix wedge
  shape; Kani INV-M1..M5 proofs 2.2 s; found a real pid-0 recursion
  bug in its first 90 s. CI staged DO.

## B. Designs where the safe-by-construction shape IS the optimization
- **Leaf-table direct unmap walk** (mm/x86_64): ownership made the
  per-page walker allocation-free; the reference-shaped fast path took
  mmap 72→27-32 µs under load and the 749 ms tail to 2-3 ms — the fix
  was removing OUR OWN cond_resched-per-chunk (a translation artifact
  the reference never had) and walking upper chains once per 2 MiB.
- **Negative-lookup memo with inode-generation stamping** (dcache):
  invalidation keyed on the parent inode's dir_mut_gen — alias-safe
  across bind mounts by construction; 475k hits / 963k stale
  normalizations, zero stale-negative events under the negstorm
  oracle.
- **R84/R85 handoff SleepMutex** (sync/mutex.rs): waiters/tokens/
  anti-starve state machine, now loom/Kani-verified (A + S-1).

## C. Correctness-by-construction
- **Checked UTF-8 on every userspace-writable string** (comm/name
  prints): userspace cannot panic the kernel printer — a 10-site
  from_utf8_unchecked family found by a stress boot and eliminated.
- **Bounded, allocation-free diagnostics**: every instrument is gated
  (one atomic load when off), budgeted, and allocation-free by rule —
  the observer cannot become the pathology.
- **Reference-parity hunting discipline**: eight root causes this
  quarter were SEMANTIC divergences from the reference kernel, each
  found by source-diffing against the 6.6 tree and fixed by adopting
  its behavior — the register's D-numbers are the ones where we
  deliberately do NOT match, with gates.

## D. Safety properties (the class mainstream kernels rent with tooling)
- Memory-safety of kernel internals by construction (no_std safe
  Rust on all non-arch paths; unsafe audited per module).
- Panic surfaces enumerated and eliminated as found (print paths,
  slot namespaces, stack-slot recycling guards).

## E. Performance landed via redesign (numbers, same-rig A/B)
- vm-op family 15→5 s per CPU-minute under OH load (B above).
- Dead-tick elimination (context-switch IRQ boundary): freed the
  clocks under exhaustion; exhaustion-class stress runs now complete
  full budgets (5.42M ops).
- SP-7 anon reclaim: flag-clear handouts + head insertion + head
  rotation (reference-opposite on all three) — examined 35→82-89%,
  swap writes +75%.
- lmbench KVM standing: 4 rows ahead of the reference on the same
  host (ctx 3.4x, pipe 2.6x, exec 1.39x, fstat 1.19x) — see
  docs/development/perf-gate-baseline.md.

## F. Observability as a first-class subsystem
- Reference-parity tracefs (tracepoint table, per-CPU rings, exact
  set_event grammar, options, snapshot) with measured disabled cost =
  ONE load per site (asm-verified) and +18-19 ns/record enabled —
  the migration ledger retires 59 bespoke switches.

## G. Pending evidence slots (placeholders until measured)
- S-2 typed binder transaction FSM (survey shortlist; scheduled with
  the OH lane).
- S-3 anykernel-style in-host test slice; S-4 writeback completion
  ring (depends on S-3).
