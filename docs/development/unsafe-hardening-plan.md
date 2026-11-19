# Unsafe hardening plan — the four orthogonal layers

User directive 2026-11-19: alongside the in-flight "primitive consolidation"
(the unsafe-slim campaign shrinking the explicit-unsafe census toward
≤5,000 block-lines), harden Rux on four more orthogonal axes — territory
isolation, machine-checked invariants, hardware runtime backstop, and
process discipline. Some of this is already done; the plan marks each item
DONE / IN FLIGHT / OPEN with its evidence pointer.

## Layer 0 — the base we stand on

**The census and its rules.** The dual metric
(`scripts/tcb_ledger.py`: explicit blocks + unsafe-fn bodies) is banked per
campaign round; `unsafe_gate` runs in CI (6,396 regions ledgered); hot
paths carry no prints (assertions only); printk is safe-and-non-sleeping
(lock-free ring, deferred wake); release builds ship with debug features
off. The consolidation itself (typed primitives: UserPtr/TaskRef/GenDisk/
KPtr/StaticRef/DmaBlock/MmioRegion, each with INV- contracts and Kani
harnesses where core) is the in-flight unsafe-slim campaign.

## Layer 1 — Architecture: unsafe is a TERRITORY, not a population (OPEN → IN FLIGHT)

**The framekernel boundary (the Asterinas lesson).** The kernel gets two
zones: **Framework** (the TCB — sched/mm/sync/kernel core + the safety
primitives; the only place `unsafe` is allowed) and **Services** (drivers,
filesystems, net, ipc — Safe Rust only, touching hardware exclusively
through the primitives' safe APIs). The boundary is enforced by the
compiler: service crates carry `#![forbid(unsafe_code)]`.

- Evidence that the direction already started: 126 files carry
  `forbid(unsafe_code)` today (mostly leaf service files). The plan
  upgrades this from incidental to **structural**: every service crate
  gets the attribute at crate level, and the D-11 typed lock keys +
  INV-contract primitives are the only crossing points.
- Sequencing (per crate, ordered by coupling): ipc/cgroup → net → fs
  leaf layers → drivers (largest; lands with the MMIO family's
  MmioRegion) → ext4. Each crate: sweep `unsafe` remnants into either a
  primitive call or a Framework move, then forbid. The census drops
  service-crate unsafe to **structurally zero** — no archaeology needed,
  the compiler is the guarantor.
- Honest note: this does not shrink the primitives' own unsafe; it makes
  the TCB line honest (everything outside the boundary is compiler-
  guaranteed) and shrinks the *audited* surface humans must read.
- Target metric: `forbid(unsafe_code)` crate count, and the census split
  into Framework/Services columns.

**The RedLeaf lesson (documented, not adopted wholesale):** type-system
isolation without address spaces is attractive but bets the whole security
property on the primitives' soundness — we take the boundary discipline
and keep the hardware backstop (layer 3) instead.

## Layer 2 — Verification: from human-argued to machine-checked (PARTIAL → EXPAND)

Current state: Kani proves core unsafe routines for all inputs; loom/exec
models check concurrency primitives; verify 1,148 proptests; Miri runs
sporadically. Expansion, in value order:

1. **Miri over the primitives' test suite (the geology check).** Miri is
   the ground truth for UB — anything Kani or a human misses, it catches
   at runtime. Run the safety-primitives' 12-test group plus the memory
   primitives' full test battery under Miri in CI (`cargo +nightly miri
   test -p kernel-safety-tests`, parallel per the 32-core directive).
   Acceptance: zero UB across the primitive suite; any hit is a
   P0-below-the-P0s (it invalidates every SAFETY contract built on it).
2. **Kani bounded proofs for the two riskiest INV families.** KPTR's
   "exactly-once free" (layout-locked Drop vs hand-written dealloc — the
   drift class the ledger documents) and MmioRegion's "no OOB window
   access" (every register access in bounds). Acceptance: harnesses
   committed with the proofs passing; the INV rows in the ledger gain a
   "machine-checked" column.
3. **Verus evaluation (one pilot, then decide).** The Atmosphere datapoint
   (3.32:1 proof-to-code ratio, 20-second verification) says full
   deduction over a shrunk TCB module is feasible. Pilot: MmioRegion's
   bounds invariant in Verus. If the proof cost stays in the promised
   class, extend to KPtr's lifetime discipline; if not, Kani-bounded
   remains the bar and the pilot's findings are recorded.

## Layer 3 — Hardware: a runtime backstop for what static analysis cannot see (OPEN)

- **Intel MPK (pkey) evaluation first.** The host is x86_64 KVM — MPK is
  available in-guest (KVM exposes PKU on modern hosts; verify with CPUID.
7.0:ECX[3,4] on the boot CPU). The shape: the primitives' internal
  metadata pages (the ledger's tracking tables, the DMA slot descriptors,
  the GRQ's bookkeeping) live in a pkey-protected region; every access
  outside the owning primitive's critical section faults in hardware.
  ERIM-class switching (WRPKRU at 11–260 cycles, <1%) guards the windows.
  Acceptance: a fault-injection probe (a deliberately escaped raw pointer,
  injected by a gated test) faults in hardware instead of corrupting.
- **SMEP-style default-deny audit (DONE this campaign — keep as the
  template):** SMEP armed (CPUID-gated, per-CPU), page-0 refused at every
  table walk. This is the hardware layer's proof-of-pattern: a hardware
  bit that converts a corruption class into a guaranteed fault.
- **MTE/CHERI: documented, not pursued** (platform-bound / research
  stage). The provenance-interaction question is tracked in the Rust
  design register as a watch item.

## Layer 4 — Process: soundness discipline (PARTIAL → SYSTEMATIC)

1. **Soundness boundary = module boundary (adopt).** Every crate that
   exposes a safe API owns its soundness end-to-end: `as_raw()/from_raw()`
   escape hatches become private by default (the round-3a primitives
   already do this; the audit extends it to all crates in the layer-1
   sweep). CI rule added to unsafe_gate: any public escape hatch without
   a per-site SAFETY row fails the gate.
2. **Unsound-abstraction pattern review (adopt from PLOS'23).** Instead of
   fixing unsafe expressions one by one, classify the recurring failure
   shapes in our INV ledger and review the corresponding primitive designs:
   the GRQ campaign's found classes (two-step release, second-authority
   trackers, IRQs-off-window deliveries, heal-style remediation) are
   exactly this — each got a protocol-level fix (owner-in-word, single
   ordered store, window-then-deliver, source-first doctrine). The new
   rule: **every third recurrence of the same INV family triggers a
   primitive-design review** (is the abstraction shape itself unsound?)
   rather than a fourth patch.
3. **The audit lane extension:** scripts/unsafe_census.py grows a pattern
   histogram column per release, so the shape drift is visible in CI.

## Sequencing (what runs when)

- NOW (in flight): the unsafe-slim consolidation rounds (3a merged;
  3b/3c finishing) — the census shrink is the prerequisite for every
  layer below.
- NEXT: Layer 2.1 (Miri over primitives — cheap, foundational), Layer 1
  crate-by-crate forbids (starting with the least-coupled service crates),
  Layer 4.1 (the escape-hatch CI rule).
- THEN: Layer 2.2 (the two Kani proofs), Layer 3 (the MPK evaluation on
  the host's CPUID), Layer 2.3 (the Verus pilot), Layer 4.2 (the pattern
  review becomes standing policy).
- Each layer lands with its own gate evidence; nothing here rewrites the
  consolidation work — every item adds an orthogonal guarantee on top.

## Success metrics

| Metric | Now | Target |
|---|---|---|
| Explicit unsafe block-lines (dual) | ~41,900 → shrinking | ≤5,000 (the consolidation goal) |
| `forbid(unsafe_code)` crates | 126 files (incidental) | every service crate (structural) |
| Primitive INV rows machine-checked | INV-tagged only | KPTR+MmioRegion Kani-proved; primitives Miri-clean |
| Hardware backstop | SMEP armed | + MPK on primitive metadata (if CPUID allows) |
| Public escape hatches | per-site rows | zero without a ledger row; CI-enforced |
| Unsound-shape recurrences | ad-hoc fixes | 3rd recurrence ⇒ primitive-design review |
