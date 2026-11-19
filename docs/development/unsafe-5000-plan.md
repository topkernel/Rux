# The ≤5,000-line / 1% unsafe plan — measured 2026-11-20

User directive: the final target is **unsafe ≤ 5,000 lines, ≤1% of the
kernel**, and the two numbers are separate deliverables. The campaign runs
as consecutive reduction rounds; every round re-banks the census
(`scripts/tcb_ledger.py`); the end state is a per-block irreducibility
proof in the ledger (which primitive owns it, why no safe shape exists,
the reference-kernel counterpart) — auditable, then the four-layer
hardening plan (unsafe-hardening-plan.md) executes on top.

## The two numbers, separated

The headline percentage is a UNION of two measures, and they shrink by
different work:

| Measure | Lines | Share of kernel/src (361,875) |
|---|---:|---|
| explicit `unsafe { … }` block bodies | 25,769 | **7.1%** |
| `unsafe fn / impl / extern` bodies | 27,097 (780 fns) | **7.5%** |
| union (overlap ~2.2k lines) | **50,630** | **14.0%** |

History: the original 12.9%/10.6% figures counted only explicit blocks —
the fn-body lines were invisible. The 11-19 dual-metric correction made
the union honest; the campaign since then already moved the block share
12.1→10.9→9.5→**7.1%**.

**Why the fn-body measure is not pointer density but DECLARATION
GRANULARITY**: of the ~18,648 fn-body lines currently measured, only a
small fraction sits inside explicit inner unsafe blocks — the rest is
unchecked merely because the enclosing function was declared `unsafe fn`.
The extreme cases on the current tree: `__schedule` (sched.rs:3918,
1,053 lines, 12 inner blocks), `new_task_at` (task.rs:2710, 766 lines, 293
raw-pointer writes), task.rs:2057 (641), ext4/mod.rs:2724 (414). The
concentration is steep: 36 functions over 100 lines hold ~8,299 of the
body lines; the ~470 small functions (≤100 lines) are what the design
intended — one narrow unsafe core per primitive.

## The reduction ladder (consecutive rounds, each re-banked)

**Round A — the 36 big unsafe fns: split into "safe body + narrow edge".
** The round-3b pattern is proven (fork.rs 925→94 block-lines; exec.rs
684→155 with the two inline page-table walks replaced by audited
helpers). Splitting the 36 converts ~8,300 fn-body lines to
compiler-checked safe code — the single biggest mechanical step. This is
the bulk of the path from 14.0% to ~10-11% union.

**Round B — semantic convergence on the worst writers.** `new_task_at`'s
293 raw-pointer writes become typed handles (TaskRef/KPtr); the same for
the other top writers. This is semantic, not just declaration.

**Round C — the 470 small unsafe fns: primitive-ize or un-sign.** Each is
either (a) a primitive core (move under an INV contract in
kernel/src/safety/), (b) convertible via an existing primitive (the
histogram: raw-deref → KPtr/StaticRef, MMIO → MmioRegion, uaccess →
UserPtr family), or (c) IRREDUCIBLE (documented per block). The
round-3/4 primitives (Ring/QueuePtr, SavedCtx/FramePtr, Reborrow) cover
the ring-API, saved-context, and reborrow families.

**Round D — the honest floor.** The irreducible classes — page-table
walks, context switch, user-memory copy, MMIO/DMA, the interrupt entry —
must be hand-written in every kernel. The reference kernel is C, so its
whole tree is TCB; Rux's difference is that these survive inside ONE
auditable Framework layer with machine-checked contracts. Round D's
deliverable is the per-block proof: which primitive owns it, why no safe
shape exists, the reference counterpart — ledger-verified, then the
≤5,000-line claim is auditable by anyone.

## Interim milestones

| Milestone | Union share | Block-body share |
|---|---|---|
| Now (2026-11-20) | 14.0% | 7.1% |
| After Round A (36 splits) | ~10-11% | ~5.5% |
| After Round B (typed handles on top writers) | ~8-9% | ~4.5% |
| After Round C (470 small fns primitive-ized) | ≤2.5% | ≤1.2% |
| End state (Round D proofs) | **≤1% (≤5,000 lines)** | — |

## The math honesty note

≤1% of a growing kernel is a moving target (the tree grows ~1k
lines/day); the plan tracks BOTH the absolute ≤5,000-line cap and the
percentage, and the irreducibility proofs are what actually bound the
number — growth lands in safe code by the layer-1 forbid policy
(unsafe-hardening-plan.md), so the unsafe count can only shrink from
here.

## Execution notes

- Rounds run as consecutive single-file-owner lanes (the no-parallel
  directive: ≤2 agents), each re-banking the census at start and end.
- Every split follows the round-3b shape: the body becomes safe Rust, the
  genuinely-unsafe edges become audited helper calls (one audit per edge,
  not per call site), assertions replace in-section prints (the
  hot-path-no-print rule).
- The boot gate catches conversion bugs before commits (round-2's
  self-recursion class); verify suite 1,148+ proptests and the Kani
  harnesses must stay green at every step.
- The four-layer hardening plan executes AFTER the ladder — its items
  (crate forbids, Miri/Kani/Verus, MPK, soundness-pattern reviews) all
  assume the shrunk surface.
