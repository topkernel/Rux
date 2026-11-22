# The Seed-Core design (晶核) — minimal TCB, every subsystem else a safe crate

Naming (2026-11-22): **Seed-Core / 晶核** — the materials-science metaphor:
the ~3,000-line hand-written core is the SEED CRYSTAL; every service
line crystallizes into safe code on the type-system lattice (forbid +
borrow checking); the compiler is the crystallization condition. The
seed is tiny yet determines the integrity of the whole crystal. The
name deliberately distinguishes from MICROKERNEL (address-space
isolation, ~1,000-cycle IPC boundaries) — this design has no address-
space boundary at all.

Status: DESIGN OF RECORD (2026-11-22, user directive: "shrink the TCB to the
minimum; ideally every subsystem becomes a crate and is safe"). This document
SUPERSEDES the end-state section of crate-plan.md (whose extraction mechanics
— the five-step flip recipe — remain binding). Execution follows this
document rung by rung; nothing lands out of order.

## 1. Goal and the one criterion

The kernel ends as **one small trusted crate plus forbid-enforced service
crates**. The criterion for every line of code is a single question:

> Can this code live under `#![forbid(unsafe_code)]`?

- YES → it belongs in a service crate (compiler-guaranteed safe).
- NO  → it belongs in `rux-core`, carries an INV contract or F-row, and is
  covered by the verification stack (Kani/Miri/Verus).

There is no third bucket. "Framework" as a catch-all for leftovers is
abolished; what remains in the core is there because it *cannot* be safe,
not because it was not yet converted.

## 2. The irreducible floor (what stays in rux-core, and why)

Every kernel hand-writes these; the reference kernel expresses them in
arch/*.S. Rux's floor, with the campaign's registered rows as evidence:

| Family | Est. dual lines | Reference counterpart | F-rows |
|---|---:|---|---|
| Trap entry/exit + signal frames | ~500 | entry.S, sigreturn | F19 |
| Context switch (3 arches) + tp/SPEL | ~250 | `__switch_to`, cpu_switch_to | F-CS-X/R/A, F-TP |
| uaccess copy bottoms | ~320 | uaccess.S / copy_user_64.S | F-UACC |
| FPU/RVV/FPSIMD save-restore | ~450 | fpu/*.S, vector.S, fpsimd.S | F-FPU/F-RV/F-AFPU |
| Page-table atomics + walks bottom | ~300 | pgtable.h atomics | F20 |
| Boot tables (GDT/TSS/8259/csr/boot.S) | ~350 | head.S et al | F-BOOT |
| CR/MSR/CSR/PIO register ops | ~250 | special_insns.h, io.h | F-CR, F-XE-PIO |
| SBI/hypercall/GIC ABIs | ~150 | kvm.c, irq-gic-v3.c | F-HVCALL, F-GIC |
| Sync primitives' interiors | ~180 | spinlock.c interiors | INV-SY/SCE rows |
| safety/ primitives themselves | ~60 | — | INV-KPTR/MMIO/... |
| Task page lifecycle (INV-TL 6 sites) | ~120 | task freeing | INV-TL |
| Timer/preemption drivers | ~60 | hrtimer cores | — |
| **Core subtotal** | **≈3,000** | | |

Additions that stay core-ADJACENT (TCB by dependency, not by unsafe):
the lock protocol implementations already audited, the printk ring, the
scheduler's switch/tp seams. The core's audit surface = the INV/F-row
registry — already 97%+ ledgered.

## 2a. Why not address-space isolation (the microkernel comparison)

The classic way to buy fault isolation is separate address spaces per
service (seL4/L4 family): a bad driver cannot corrupt kernel memory
because it cannot address it. The price is the boundary — every
cross-service operation is an IPC through a context switch, ~1,000
cycles on the best fastpaths, multiplied along fault->driver->server
chains (the Mach lesson; L4's fastpaths recovered much but not all of
syscall-heavy paths).

This design buys the SAME memory-safety property statically, at zero
runtime cost: a `#![forbid(unsafe_code)]` crate CANNOT EXPRESS a wild
pointer, use-after-free, or data race — the Rust type system provides
what address spaces provide in a C microkernel, without a runtime
boundary. Cross-crate calls are plain function calls (LTO inlines; the
M3 flip verified zero behavior change with the dispatch tables live).
This is the RedLeaf/Framekernel line: language isolation without
address spaces, keeping the hardware backstop (below) instead of
betting everything on the type system.

What remains true for both designs: logic bugs (deadlocks, wrong
decisions, leaks) pass through; only memory-safety classes differ.

**The hardware backstop — MPK (Layer 3 of the safety plan, measured):**
for defense-in-depth BELOW the type layer, Intel protection keys give
page-grain hardware domains inside the ONE address space: WRPKRU
transitions measured at **47-49 cycles** on this host (100k TSC-timed
ops; the ERIM native band is 20-90), vs ~1,000+ for an address-space
switch with TLB flush. Protected pages fault in hardware outside their
window (fault-injection proven: the identical write lands with the key
enabled and takes a hardware #PF with it disabled). The mpkguard
framework is landed behind `mpkguard=1` (default off): boot arm with
explicit keys, PKRU zones, context-switch re-arm, tagged-page
discipline, the escape self-test. Phase-1 target: the vring AVAIL
sections (~2.4k windows/s at pbench fs rates — the <1% budget holds);
GRQ (~160k/s) is phase-2 candidate by measured frequency.

The end-state defense stack, each layer at its measured cost:
1. TYPE layer (0 cycles): forbid crates — service columns structurally
   immune to memory unsafety.
2. HARDWARE layer (~50 cycles/window, <1%): MPK-protected core
   metadata — even a type-system soundness hole or smuggled unsafe
   meets a hardware fault, not silent corruption.
3. VERIFICATION layer (0 runtime): Kani/Miri/Verus over the ~3,000
   core lines — the remaining hand-written unsafe is machine-checked.

A microkernel spends ~1,000 cycles per boundary to buy (1); this design
buys (1)+(2)+(3) for 0+~50+0.

## 2b. Lineage and deltas (vs Asterinas Framekernel, vs RedLeaf)

Same family — language-isolated kernels — three different bet structures:

| Axis | Asterinas (Framekernel, USENIX ATC'25) | RedLeaf (PLOS'23) | This design |
|---|---|---|---|
| Isolation enforced by | Convention + review inside ONE kernel crate | The type system, via inter-domain zero-copy RPC | The COMPILER at crate granularity: per-crate `#![forbid(unsafe_code)]` (negative-probe verified) + Cargo's package DAG refusing cycles (dependency-direction acyclicity machine-checked) + a CI ratchet on the attribute set |
| Boundary shape | Intra-binary function calls | Typed RPC channels over domain boundaries | Plain cross-crate function calls, LTO-inlined — thinner than RPC by a whole protocol layer |
| Type-soundness failure mode | No backstop | No backstop (their stated bet) | MPK hardware backstop: pkey-tagged core metadata pages fault outside their window (WRPKRU at 47-49 measured cycles; fault-injection proven) |
| TCB form | The framework zone (review-audited) | A small verified runtime | ~3,000 lines ITEMIZED (each with reference counterpart + F-row id), covered by the verification stack (Kani all-input proofs, Miri zero-UB, Verus narrow-adopt for unbounded arithmetic lemmas) |
| Unique here | — | — | The census/INV/F-row ledger (97%+ artifact coverage: the audit surface is a ledger, not a code read); the policy/core splits S1/M1 (decision logic exits the core, mechanism stays); the binding measure-first flip ladder with shelving-as-success |
| Where they lead | Full Linux syscall surface shipped earlier | Complete zero-copy-RPC formalism | — (honest) |

## 3. The end-state topology

```
rux-core          (≈3,000-3,500 dual lines; the entire TCB)
  arch/{x86_64,riscv64,aarch64} entry/switch/uaccess/pt/asm zones
  safety/  sync-primitives  task-lifecycle  timer-core  printk-ring
  ← verification stack: Kani 12/12, Miri 0-UB, Verus narrow-adopt

rux-sched         forbid-safe  (CFS/RT/fairness/wakeup/deadline policy)
rux-mm            forbid-safe  (VMA tree, mmap/COW/fault DECISIONS, brk)
rux-syscall       forbid-safe  (DONE at M3; the pattern-proven first rung)
rux-fs        forbid-safe  (VFS glue, page cache, leaf filesystems)
rux-ext4          forbid-safe  (deepest fs consumer)
rux-net           forbid-safe
rux-drivers       forbid-safe
rux-ipc           forbid-safe
rux-dfx-svc       forbid-safe  (subject to §7 retirement review)
```

Cargo refuses cycles: services → core (and → lower services) only; the
constraint is machine-checked once the rungs land.

**The census effect**: every service crate's unsafe count is structurally
zero (compiler-guaranteed). The whole-kernel dual number BECOMES the core
size. See §6 for the stage math.

## 4. The two policy splits (the new rungs this design adds)

The campaign's earlier framing kept sched and mm whole in "the framework".
Both bundle decidable policy over a small unsafe core; the splits separate
them.

### Rung S1 — sched policy / core split

**S1 is the first compiler-enforced implementation of MECHANISM/POLICY
SEPARATION** (the classic OS design principle): the MECHANISM — the
low-level state-changing operations (context switch, the global run
queue's locking protocol, the current-task pointer's register read,
task-slot allocation and reclamation) — directly maintains the kernel's
data-structure invariants and stays in the core crate under audit; the
POLICY — the decisions built on the mechanism (which task runs next,
which CPU a wake-up is placed on, when to throttle) — accesses data only
through the mechanism's type-safe interface and moves to a separate
service crate under forbid. Traditionally this separation was maintained
by code review alone; here the compiler statically guarantees it: any
unsafe use in policy code is a compile error. The callback-function
table and the private-interface API constitute the mechanism's complete
calling contract published to the policy. M1 applies the same
separation to mm.

- MOVES to `rux-sched` (forbid): CFS/RT/deadline decision logic,
  fairness math, wakeup placement policy, load-balancing decisions,
  cgroup scheduling policy, idle balancing, the census/diagnostic walks.
- STAYS in core: `__switch_to` + tp derivation + stack switch (F-CS/F-TP),
  the GRQ lock primitive (owner-in-word), the terminal-switch protocol
  (reapdrain's R85/pacing machinery — page-identity TCB), NEED_RESCHED
  atomics, the runqueue's intrusive links under the lock discipline.
- THE SEAM: policy calls core via safe APIs — `switch_to(next)`,
  `current()`, `grq_lock()/unlock()`, `enqueue/dequeue(task_ref)` — the
  typed handles (TaskBorrow/TaskRef) already exist for exactly this.
- MEASURE FIRST (the standing rule): count policy→core and core→policy
  edges on the current tree before any move. Abort criterion: if the
  policy's decision logic reads raw task pointers in more than a handful
  of sites that cannot ride `task_shared`, the rung shelves with numbers.

### Rung M1 — mm policy / core split

- MOVES to `rux-mm` (forbid): the VMA tree and its lock protocol's
  policy side, mmap/munmap/mprotect DECISIONS, COW/fork mapping policy,
  fault ROUTING (which arm), brk policy, oom/kill policy, compaction
  decisions, kswapd policy.
- STAYS in core: page-table walks + PTE atomics (PtTable primitives),
  the uaccess bottoms, the fill/copy frame primitives (FrameWindow,
  INV-PC-WIN), the VMA-find lock rule's primitive side (find under
  read-guard as a core primitive — the defect-C lesson makes this seam
  explicit), buddy allocator (already Framework floor), vmemmap/page_desc.
- THE SEAM: policy consumes `PtTable`, `FrameWindow`, `vma_find_guarded`,
  `map/unmap_page` safe APIs. The fault path becomes: core traps →
  policy decides → core executes the primitive.

### The existing rungs (unchanged, per crate-plan.md)

- Rung 1 syscall: M1-M3 (in flight; State B = file deletion closes it).
- Rung 2 net: measured at ~35 reverse sites (~2 lane-days post-seam).
- Rung 3 fs-svc, Rung 4 drivers, Rung 5 ext4: measured first, per the
  standing method; a rung whose reverse surface will not decompose is
  SHELVED with the numbers recorded in this document's §8.

## 5. What happens to the edges files

The per-subsystem `edges.rs`/`unsafe_edges.rs` files dissolve by rung:
each of their functions is either (a) a call into a core primitive — the
edge fn becomes a thin re-export and then dies, or (b) genuinely local
unsafe — it MOVES into rux-core under its INV row. At the end state there
are no per-subsystem edges files; there is the core's registry.

## 6. The census math (honest, staged)

Current: ~10,300 dual (whole kernel, single-crate census).

| Stage | Lands | Expected dual |
|---|---|---:|
| Now | — | ~10,300 |
| + Rung 1 close (syscall crate, files deleted) | census column split begins | ~10,000 (unchanged mass; structural change) |
| + Rung 2 net | net column → 0 | −~200 |
| + S1 sched-policy split | sched policy column → 0 | −~300 |
| + M1 mm-policy split | mm policy column → 0 | −~250 |
| + Rung 3 fs-svc + 5 ext4 | fs service columns → 0 | −~2,400 |
| + Rung 4 drivers | drivers column → 0 | −~370 |
| + dfx retirement review (§7) | dead instruments retire | −600~1,000 |
| **End state** | **= rux-core size** | **≈4,000-4,700** |

The ≤5,000 target is met with margin iff the dfx review retires its
dead instruments; without it the landing is ≈5,000-5,400 and the review
becomes decisive. The percentage target (≤1%) is met trivially at any
of these values.

## 7. The dfx retirement review (feeds §6)

Per-instrument adjudication: hunt closed → instrument retires (delete);
hunt alive → instrument moves to `rux-dfx-svc` behind its flag (forbid if
its unsafe can ride core primitives; else core-adjacent with INV rows).
Input: L4's alive-call-site census (scribble 49 sites, leakorigin 26,
r83hunt 8, wildwatch 5, treehunt 1). Each verdict one line in §8's table.

## 8. The execution ladder (order is binding; each rung = measure → design seam → flip → forbid → battery)

1. **R1 close** (in flight): syscall State B — delete in-lib files, full
   battery, negative probe. Gate: triple-arch, make test, fio 3-case,
   smoke; census columns begin.
2. **S1 measure** (next): sched policy/core edge count (one day, no code).
   Then seam design → policy crate flip → forbid → battery incl. P4
   storm + ctxsw pbench (guarded domain!).
3. **M1 measure**: mm policy/core edges. Then as S1 (battery incl. the
   defect-C pair + mmap LTP rows).
4. **R2 net**: the measured 35-site relocation + flip.
5. **R3 fs-svc + R5 ext4**: the biggest mass; measured per crate-plan.
6. **R4 drivers**: last service rung.
7. **dfx review**: per §7.
8. **Core audit freeze**: the final INV/F-row registry pass — every core
   line under a row; the per-block irreducibility proof (the D-round
   deliverable) then covers exactly rux-core.

Every rung's gates: the standing set (triple-arch build, make test,
unsafe_gate, print ratchet, smoke) + the rung's domain battery + the
forbid negative probe at crate scope. A rung that fails its battery
twice halts the ladder (no piling changes on an unstable base).

## 9. Risks and honest limits

- **The core is still ~3,000 lines of hand-written unsafe.** The design
  minimizes it to the reference kernel's own floor class; it does not
  eliminate it. The verification stack is the compensating control.
- **Policy/core seams can be wrong** (the defect-C lesson: seams between
  lock domains are where races live). Each split's seam gets the fault
  battery, not just the suite.
- **Hot-path cost**: policy-in-crate means cross-crate calls on scheduler
  and fault paths. Mitigation: LTO + inlining across the workspace is
  already the build shape; the ctxsw/fault pbench rows gate each split.
- **Rung shelving is success, not failure**: a measured non-decomposable
  surface keeps that subsystem core-side with its numbers recorded here.

## 10. Relationship to existing documents

- crate-plan.md: extraction mechanics remain binding; its end-state
  section is superseded by §3 here.
- unsafe-5000-plan.md Part I: the ladder's D-round proof now targets
  rux-core (§8 step 8). Part II status board unchanged.
- safety-and-performance.md: the INV/F-row registry becomes the core's
  audit surface; no format change.
