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


---

# Part II — the four orthogonal hardening layers

(Formerly the separate unsafe-hardening-plan.md — same campaign, the
second half: these layers do not shrink the count; they add orthogonal
guarantees on top of the ≤5,000-line end state. User directive
2026-11-19/20: one plan.)

## Layer 0 — the base we stand on

**The census and its rules.** The dual metric
(`scripts/tcb_ledger.py`: explicit blocks + unsafe-fn bodies) is banked
per campaign round; `unsafe_gate` runs in CI (6,396 regions ledgered);
hot paths carry no prints (assertions only); printk is safe-and-non-
sleeping (lock-free ring, deferred wake); release builds ship with debug
features off. The consolidation itself (typed primitives:
UserPtr/TaskRef/GenDisk/KPtr/StaticRef/DmaBlock/MmioRegion, each with
INV- contracts and Kani harnesses where core) is the in-flight
unsafe-slim campaign (Part I's ladder).

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
3. **Verus evaluation (one pilot, then decide) — PILOT COMPLETE
   2026-11-21, verdict recorded below.** The Atmosphere datapoint
   (3.32:1 proof-to-code ratio, 20-second verification) says full
   deduction over a shrunk TCB module is feasible. Pilot: MmioRegion's
   bounds invariant in Verus.

   **PILOT RESULT (kernel/safety-tests/verus/pilot_mmio.rs; Verus
   0.2026.11.02 prebuilt + rustup 1.99.1 toolchain, ~10 min setup,
   no source build):** 4 proofs verified, 0 errors, 1.9 s — an order
   under the 20 s pitch. Three theorems over a spec twin of
   check()'s arithmetic: T1 no-OOB (admitted access spans lie inside
   the window, unbounded inputs), T2 wrapping-add alignment closure
   (the conviction-#1 fix's justification: 2^64 % w == 0 for
   power-of-two widths — a lemma Kani CANNOT check, being bounded),
   T3 admit-closure (addr reconstructs off). Ratio: 74 proof-file
   lines to ~12 lines of mirrored impl arithmetic ≈ 6:1 raw, ≈ 3.7:1
   counting only theorem+spec lines — the Atmosphere class.

   **VERDICT: Kani-bounded stays the standing bar; Verus is adopted
   NARROWLY, for the unbounded-arithmetic lemma class** (modulo/wrap
   identities, bound derivations — properties that hold for ALL
   inputs, where a bounded checker can only sample). The raw-memory
   layer (volatile derefs) stays with Kani/Miri — Verus has no
   executable model for it, and forcing it would distort the impl
   (evaluation question 1: avoided by the spec-twin scope; the
   twin↔impl correspondence is by line-mirroring and is the pilot's
   honest weakness — a follow-up stitch would be a Kani harness
   asserting twin/impl agreement on symbolic inputs). Composition
   (question 3): complementary, not fighting — the two tools own
   disjoint property classes over the same primitive. KPtr's
   lifetime discipline: NOT extended this round — it is heap-liveness
   (Kani's memory model already owns exactly-once); the Verus-shaped
   share would be the layout-identity lemma, queued as optional.

## Layer 3 — Hardware: a runtime backstop for what static analysis cannot see (OPEN)

- **Intel MPK (pkey) evaluation first.** The host is x86_64 KVM — MPK is
  available in-guest (KVM exposes PKU on modern hosts; verify with
  CPUID.7.0:ECX[3,4] on the boot CPU). The shape: the primitives'
  internal metadata pages (the ledger's tracking tables, the DMA slot
  descriptors, the GRQ's bookkeeping) live in a pkey-protected region;
  every access outside the owning primitive's critical section faults in
  hardware. ERIM-class switching (WRPKRU at 11–260 cycles, <1%) guards
  the windows. Acceptance: a fault-injection probe (a deliberately
  escaped raw pointer, injected by a gated test) faults in hardware
  instead of corrupting.
- **SMEP-style default-deny audit (DONE this campaign — keep as the
  template):** SMEP armed (CPUID-gated, per-CPU), page-0 refused at every
  table walk. This is the hardware layer's proof-of-pattern: a hardware
  bit that converts a corruption class into a guaranteed fault.
- **Intel MPK (pkey) evaluation: ADOPT-READY (measured 2026-11-21,
  probe at kernel/src/arch/x86_64/mpk.rs, cmdline `mpkprobe=1`,
  default off).** All four deliverables landed on the primary rig
  (9950X host, KVM, `-cpu max`): (1) CAPABILITY — CPUID.7.0:ECX
  bit3 (PKU support) reads 1 in the guest; bit4 (OSPKE) flips to 1
  the moment the kernel arms CR4.PKE (the probe's own verification —
  note the bit semantics: bit3 is the HARDWARE bit, bit4 the
  CR4.PKE mirror the OS sets; reading bit4 as the hardware bit was
  the first probe's false negative); (2) COST — WRPKRU round-trip
  **47-49 cycles/op** (100k ops, TSC-timed, under KVM), inside
  ERIM's native 20-90-cycle band and trivially inside the <1%
  overhead budget (a 47-cycle switch on microsecond-scale
  primitive windows); (3) FAULT-INJECTION — a 4K page mapped with
  pkey=1 (PTE bits 62:59) on the ACTIVE root at a fresh
  user-range VA: with PKRU key1 enabled the raw-pointer write
  LANDS (value verified); with key1 AD|WD set the SAME write takes
  a hardware #PF, converted by the uaccess exception-table window
  to a return code — the backstop is a proven fault, not an
  argued one. Two probe-engineering findings for the adopter: the
  protected page must be installed on the ACTIVE CR3 root (a
  static-root install left it absent; the kernel-VA fault path
  re-mapped it on its own NX template — the template-override, NOT
  a PKU failure), and the fixmap window is huge-leaved at PMD (a
  4K walk there resolves garbage — use a fresh user-range VA);
  (4) QEMU's PKRU reset value is 0x0 on this host (Linux's
  0x55555554 assumption does not hold under KVM) — any adopter
  must SET the AD bits for keys 1-15 explicitly at arm time, not
  rely on the reset. The design shape from this plan stands
  (metadata pages in a pkey-protected region, WRPKRU windows at
  the primitive boundaries); the probe's ex_table-bracketed access
  window is the template for the primitives' internal accesses.
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

## Part II status board (updated 2026-11-21 — user directive: remaining layers moved up)

| Layer | Item | Status |
|---|---|---|
| 1 | Framekernel boundary: per-module forbids | **191 modules** compiler-enforced, CI-ratcheted |
| 1 | Crate flips | **IN FLIGHT** — syscall pilot ADOPTED (syscall-crate-pilot.md: reverse surface 88 sites, ~90 relocations); the flip executing; net measured next |
| 2.1 | Miri over primitives | **DONE** — 12/12 zero UB (kernel-safety-tests, verbatim #[path]) |
| 2.2 | Kani proofs (KPTR/Mmio) | **DONE** — 12/12 verified; two checker-self convictions fixed |
| 2.3 | Verus pilot | **DONE** — 4 theorems/1.9s; narrow-adopt verdict recorded |
| 3 | MPK evaluation | **IN FLIGHT** — capability probe + ERIM-cost + fault-injection prototype assigned |
| 3 | SMEP default-deny | **DONE** (the template) |
| 4.1 | Escape-hatch CI rule | **DONE** — escape_hatch_check in unsafe_gate |
| 4.2 | Pattern-review policy | **DONE** (below) |
| 4.2 | Census pattern histogram | TODO (unsafe_census.py grows the column) |

**Layer 4.2 standing policy (adopted 2026-11-21):** every THIRD recurrence
of the same INV defect family triggers a PRIMITIVE-DESIGN REVIEW (is the
abstraction shape itself unsound?) rather than a fourth site-patch. The
GRQ campaign's four families (two-step release, second-authority
trackers, IRQs-off-window deliveries, heal-style remediation) are the
precedents — each got a protocol-level fix. The reviewer of record is
the lane that lands the third recurrence; the review lands in
safety-and-performance.md next to the family's INV rows.

## The combined sequencing

- NOW (in flight): Part I's consolidation rounds (3a merged; 3b merged;
  3c merging; Round A of the ladder next) — the census shrink is the
  prerequisite for every layer below.
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
| Explicit unsafe block-lines (dual) | 25,769 (7.1%) | ≤5,000 / ≤1% (the Part I ladder + per-block proofs) |
| `forbid(unsafe_code)` crates | 126 files (incidental) | every service crate (structural, layer 1) |
| Primitive INV rows machine-checked | INV-tagged only | KPTR+MmioRegion Kani-proved; primitives Miri-clean (layer 2) |
| Hardware backstop | SMEP armed | + MPK on primitive metadata (layer 3, if CPUID allows) |
| Public escape hatches | per-site rows | zero without a ledger row; CI-enforced (layer 4) |
| Unsound-shape recurrences | ad-hoc fixes | 3rd recurrence ⇒ primitive-design review (layer 4) |
