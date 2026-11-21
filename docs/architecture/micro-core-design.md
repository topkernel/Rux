# The micro-core design — minimal TCB, every subsystem else a safe crate

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

## 3. The end-state topology

```
rux-core          (≈3,000-3,500 dual lines; the entire TCB)
  arch/{x86_64,riscv64,aarch64} entry/switch/uaccess/pt/asm zones
  safety/  sync-primitives  task-lifecycle  timer-core  printk-ring
  ← verification stack: Kani 12/12, Miri 0-UB, Verus narrow-adopt

rux-sched-policy  forbid-safe  (CFS/RT/fairness/wakeup/deadline logic)
rux-mm-policy     forbid-safe  (VMA tree, mmap/COW/fault DECISIONS, brk)
rux-syscall       forbid-safe  (DONE at M3; the pattern-proven first rung)
rux-fs-svc        forbid-safe  (VFS glue, page cache, leaf filesystems)
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

- MOVES to `rux-sched-policy` (forbid): CFS/RT/deadline decision logic,
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

- MOVES to `rux-mm-policy` (forbid): the VMA tree and its lock protocol's
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
