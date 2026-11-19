# Security design — layered defense for a monolithic kernel

*Status: design document, 2026-11-01. Companion documents:
[kernel-layers.md](kernel-layers.md) (the layer taxonomy),
[design-divergences.md](design-divergences.md) (the divergence register),
[../development/tcb-ledger.md](../development/tcb-ledger.md) (the generated
unsafe accounting), [../development/security-posture.md](../development/security-posture.md)
(the per-surface posture ledger).*

Rux is a **monolithic kernel**: there is no microkernel message
boundary to hide behind. Its isolation story is therefore layered, and
each layer is chosen so that the *cheapest* mechanism carries the most
weight:

| Layer | Mechanism | What it defends | Cost |
|---|---|---|---|
| L1 | Language-level isolation (types + `forbid(unsafe_code)`) | memory-safety of the bulk of the kernel | ~0 (compile-time) |
| L2 | Syscall boundary (uaccess + entry validation) | the user/kernel boundary | ~55 ns per AC bracket |
| L3 | Privilege model (43 capabilities, seccomp) | authority of user code | per-check atomics |
| L4 | Hardware aids (SMAP/MPK/MTE) | the residual `unsafe` interior | 20–26 cycles (MPK) |
| L5 | Formal verification of the TCB | the Framework layer's correctness | offline |
| L6 | Runtime mitigation | known vulnerabilities after release | ~1% CPU (eBPF-class) |

The ordering principle: **type system first, hardware second,
verification third, runtime mitigation last**. Each layer below states
what is already true in the tree, what is missing, and the measurable
target that closes the gap.

---

## 0. Where the tree stands today (2026-11-01, generated numbers)

From `scripts/gen_tcb_ledger.py`:

- unsafe tokens **outside** the Framework layer (service code): **3158**
- Framework-layer tokens: 1340 · diagnostics: 193 · test: 80
- service files: 179 — `forbid(unsafe_code)`: **47** · already unsafe-free:
  **56** · still unsafe-bearing: **123**
- SAFETY-comment coverage over unsafe constructs: **3207 / 5009 (64 %)**
- hottest modules by unsafe tokens: syscall 722, fs 556, ipc 295,
  process 277, net 223, mm 219, sched 207

Already in place, and load-bearing:

- **uaccess discipline** — every user pointer crosses through
  `arch::uaccess`; the x86_64 port runs with **CR4.SMAP** and the
  usercopy cores bracket their copies with EFLAGS.AC sequences
  (D-18). The kernel has no identity window to read physical memory
  through a user address (D-17), so a bad pointer faults instead of
  returning kernel bytes.
- **Layered file-level `forbid(unsafe_code)`** — 47 service files
  enforce it at the compiler; the Framework layer (mm/sync) is the
  audited interior.
- **Privilege model** — 43 capabilities with a Linux-like LSM hook
  point, `exec` transitions, seccomp (STRICT/FILTER modes).
- **Formal verification** — 157 Kani harnesses, 1104 host proptests, 4
  SPIN models, Miri on the verify crate; mirror-sync is enforced so
  proofs stay pinned to the code they cover.

Known gaps this document addresses:

- **G1** SMAP never arms on real hardware: `enable_smap()` tests
  **CPUID leaf 7 EDX[20]**; the SMAP capability bit is **EBX[20]**. The
  emulators we test on happen to report it in EDX, so the defect is
  invisible in CI and the protection is absent on silicon.
- **G2** 123 service files still carry `unsafe`; the largest clusters
  are syscall (722 tokens), fs (556), ipc (295).
- **G3** SAFETY-comment coverage is 64 % — the uncovered quarter is
  exactly the region where a reader has no invariant to check.
- **G4** no hardware memory-key discipline: the `unsafe` interior can
  read any kernel page it can reach.
- **G5** no runtime mitigation channel: a shipped vulnerability can
  only be fixed by a rebuild.

---

## 1. Path one — language-level isolation domains

**Idea.** Isolation without the MMU: a "domain" is a module whose
memory safety is guaranteed by the type system, communicating with the
rest of the kernel through typed interfaces. A domain that fails
cannot corrupt its neighbours, because the compiler already proved it
cannot write outside its own data.

This is the path that fits a monolithic kernel best: a domain switch is
an ordinary function call — no page-table switch, no TLB cost, no
context switch.

**How Rux already does it.** The layer map is the mechanism:
`kernel-layers.md` classifies every module as *framework*
(the `unsafe` interior: mm, sync), *service* (everything that may be
made safe), *diagnostics* (dfx) or *test*. A service file that carries
no `unsafe` gets `#![forbid(unsafe_code)]` — the compiler then
enforces the boundary for it, permanently.

**What to do next (targets, in order of payoff):**

1. **Close G1 immediately.** Fix the CPUID register, and make the boot
   log state the *actual* SMAP state ("CR4.SMAP enabled" only when
   EBX[20] said so). Add a boot-time assertion that the kernel never
   runs with SMAP silently off on hardware that has it.
2. **Grow the safe-file frontier.** The mechanically easiest wins are
   the files whose unsafe tokens are mostly in *audit helpers* rather
   than in the hot path: `syscall/` (722 tokens, 9 files) and `ipc/`
   (295, 6 files) have the largest per-file clusters; each file that
   drops to zero unsafe earns `forbid(unsafe_code)` and is retired
   from the TCB ledger. Concrete goal: **forbid files 47 → 90**,
   unsafe-bearing service files 123 → 60, in two quarters.
3. **Graded unsafe auditing (G3).** Replace the flat "unsafe token"
   count with a **graded** one: an `unsafe` block that only reads
   kernel-owned data through a checked accessor is *audit-class 1*; a
   block that dereferences a raw pointer with a hand-rolled invariant
   is *class 3*. The ledger then reports "class-3 tokens in service
   code" as the number that actually predicts field bugs — and the
   SAFETY-comment coverage target rises from 64 % to **95 %** on class
   2–3 constructs, where the comment is the audit record.
4. **Capsule-shaped services.** Where a subsystem needs raw hardware
   access (virtio queue rings, GPU register files, a DMA descriptor
   path), wrap it in a **capsule**: a `#![forbid(unsafe_code)]` façade
   exposing a typed API to the rest of the kernel, with the raw
   register access confined to one module inside the capsule. The
   existing `arch/uaccess_safe.rs` (typed user-memory capsules) is the
   template; the DMA/virtio paths are the next candidates.

**Cost.** Compile-time only for (1)–(3); for (4) the capsule façade
adds one indirection per call — measurable only on the hottest DMA
path, which is why capsules wrap the *interface*, not the data path.

**Reference lineage.** The capsule discipline (a `#![forbid(unsafe_code)]`
boundary around a service, with the unsafe core kept small and audited)
is the Tock pattern; isolation domains with recoverable private heaps
are the RedLeaf pattern; language-mediated subsystem isolation with
restricted inter-domain communication is the Alien pattern. Rux's
take is the capsule half — it does not attempt domain-switching
mechanisms, because a monolithic kernel has no second kernel to switch
to.

---

## 2. Path two — hardware-assisted fine-grained protection

**Idea.** The type system cannot see inside an `unsafe` block, and it
cannot see side channels. Hardware can bound the blast radius of the
`unsafe` interior without costing a page-table switch.

**What Rux has, what is missing:**

| Mechanism | Availability here | State | Action |
|---|---|---|---|
| SMAP (x86_64) | x86_64 (KVM + silicon) | implemented; **CPUID bug G1** | fix the register, re-verify, then treat "SMAP armed" as a boot invariant |
| MPK / pkeys | x86_64 silicon (not emulated) | **not implemented** | candidate for the Framework interior: give the audit-class-3 subsystem a key, so a wild write inside it cannot reach the rest of the kernel |
| MTE (tagged pointers) | aarch64 silicon | **not implemented** | candidate for the aarch64 port: tag allocations, catch use-after-free in the slab/zone paths |
| CHERI | research hardware | not a target | out of scope — noted as the long-horizon capability-based alternative |
| RISC-V pkeys | not in the base ISA | n/a | the riscv64 port relies on the type system + SMAP-equivalent discipline (no SUM) |

**Where MPK would pay.** The Framework layer (mm, sync) is the one
place where a wild write is catastrophic. With MPK, the kernel could
give each Framework subsystem (zone allocator, VMA manager, page cache)
its own key and run it with a key that cannot address the rest of the
kernel heap. Cost on silicon: 20–26 cycles per key switch, which is
irrelevant for a per-subsystem-entry granularity and irrelevant for
the hot paths (the allocator's fast path never changes keys).

**Constraint to state plainly.** MPK is not emulated by the emulators
in this rig, so any MPK work must be gated like the `dfx=` probes:
**off by default, enabled explicitly, with the unsafe-interior
segmentation verified by construction** (the keys are a compile-time
partition, not a runtime policy). The same applies to MTE on aarch64.

**Cost.** SMAP: one AC bracket per usercopy (~55 ns measured in the
filerd decomposition). MPK: a key switch per Framework entry.
MTE: tag store/load per allocation.

---

## 3. Path three — pragmatic formal verification of the TCB

**Idea.** Verify the small `unsafe` core exhaustively instead of the
whole kernel. This is tractable because the core is small by
construction.

**What Rux does today.** The verify crate mirrors the kernel's
algorithms and proves them: 157 Kani harnesses (bounded model checking
of core `unsafe` code for all inputs), 1104 host proptests, 4 SPIN
models for concurrency, Miri for undefined behavior. The mirror-sync
checker (`scripts/verify_sync_check.py`) makes a drifted proof fail the
build — so a proof cannot silently stop covering its code.

**What to add.**

1. **Verus for the Framework core.** The Rux Framework is
   mm/sync/arch primitives: the allocator's invariants, the VMA
   manager's tree operations, the refcount/page-flag algebra. These are
   exactly the kind of code a full-functional-verification tool
   handles well, and the proof-to-code ratio reported for comparable
   projects (order of a few-to-one) is achievable here because the
   Framework is deliberately small. Start with the **page-flag and
   refcount algebra** (small, pure, and the source of several historic
   corruption families), then the allocator's free-list invariants.
2. **RustBelt as the trust anchor.** A formal semantics for the
   Rust subset the kernel relies on is what makes "the Safe Rust parts
   are actually safe" a claim rather than an assumption. It is a
   research dependency, not a build dependency: cite it as the
   justification for the layer discipline, keep the build on plain
   rustc.
3. **Keep the mirror discipline, extend it.** Every Framework
   primitive should have (a) a Kani harness, (b) a proptest, and
   (c) — as the framework shrinks — a Verus function spec. The ledger
   should report the three-way coverage, so a primitive cannot be
   "verified" by a mirror that drifted.

**Cost.** Verus is a separate toolchain (not rustc); the proofs are
offline, so they gate merges rather than every build. Engineering
cost is real but bounded by the Framework's size.

---

## 4. Path four — TCB reduction and runtime mitigation

**Idea.** Shrink what has to be trusted, and keep a mitigation channel
for what ships anyway.

**TCB reduction, continued:**

- **Allocation discipline.** The historic failure modes include
  allocation failure inside a lock (the wedge family) and
  exhaustion-driven fake-OOM kills. The direction is: no dynamic
  allocation on paths that hold locks (the kernel already has the
  `try_reserve` discipline in the hot allocation sites), and a
  reserved-margin policy so allocation failure degrades a subsystem
  instead of the kernel.
- **Graded audit instead of blanket audit** (see path 1.3) — the
  ledger's headline number should become "class-3 tokens", i.e. the
  subset where a human invariant is the only protection.
- **Erase-and-reuse discipline.** Page poisoning on free and
  re-allocation turns a stale-pointer read into a detectable fault
  rather than silent corruption; the framework already carries the
  scribble hunter for task control words, which is the same idea
  applied to the hottest structures.

**Runtime mitigation channel.** A mechanism that can change behavior
without a rebuild: eBPF-class programs attached to the kernel's own
event points (the tracepoint table already exists — D-15), able to
apply hot patches such as object-address randomization, an audit
filter, or a capability-scope narrowing. Measured overhead for the
comparable class of mitigation: ~1 % CPU and ~0.9 % memory. It is a
**mitigation**, not a mechanism: it exists so that a shipped
vulnerability has a same-day lever while the structural fix ships.

---

## 5. The combination

The four paths compose into one defense, each layer carrying what the
layer above cannot:

```
        user space (untrusted)
              │  syscall boundary: uaccess + AC brackets + SMAP   [L2/L4]
              ▼
   ┌──────────────────────────────────────────────┐
   │  service layer: capsules, #![forbid(unsafe_code)] │  [L1]
   │  90 files · typed façades · graded audit       │
   └──────────────────────────────────────────────┘
              │  typed calls only (no raw pointers across the edge)
              ▼
   ┌──────────────────────────────────────────────┐
   │  Framework (the TCB): mm, sync, arch primitives│  [L1/L4/L5]
   │  1340 unsafe tokens · class-3 audited         │
   │  MPK-keyed per subsystem (where available)     │
   │  Kani + Verus proofs pinned by mirror-sync     │
   └──────────────────────────────────────────────┘
              │  privilege model: 43 caps + seccomp   [L3]
              ▼
        kernel-owned state
```

Order of work, by benefit-per-engineering-hour:

1. **G1** — the SMAP CPUID register fix (one line + a boot assertion).
   A hardware protection that is silently off is worse than no
   protection, because the design assumes it.
2. **Capability shaping** — the privilege model is the L3 layer and is
   the cheapest place to add authority boundaries: audit which of the
   43 capabilities are actually consulted on each path, and narrow the
   ones that are not (a capability that is never checked is a
   capability that grants nothing).
3. **Capsule frontier** — the file-level `forbid` campaign (47 → 90),
   prioritized by unsafe density.
4. **Graded audit + SAFETY coverage to 95 %** — this is what makes the
   TCB number meaningful.
5. **Verus on the Framework core** — the page-flag/refcount algebra
   first.
6. **MPK/MTE as opt-in segments** — gated like the dfx probes, verified
   by construction, never on by default.
7. **Runtime mitigation channel** — last; it is a mitigation.

## 6. How each layer is measured

| Layer | Metric | Source | Cadence |
|---|---|---|---|
| L1 | unsafe-bearing service files; class-3 tokens; SAFETY coverage | `gen_tcb_ledger.py` | per change |
| L1 | safe-file frontier | `forbid(unsafe_code)` count | per change |
| L2 | EFAULT correctness; no identity reads | `uafault` probe; PF-NOCTX beacon | nightly boot |
| L3 | capability checks per path | posture ledger | per change |
| L4 | SMAP/MPK/MTE armed state | boot log + a probe | per boot |
| L5 | harness coverage of Framework primitives; mirror-sync | `verify_sync_check.py`; Kani/Verus runs | per change / nightly |
| L6 | mitigation applied; overhead | tracepoint attach stats | on demand |

The merge gate for a security-track change: the layer it touches must
report its metric before and after, and the tree's standing numbers
(regenerated) must not regress.
