# Memory safety architecture — the design in one place

*Status: umbrella document, 2026-11-20. It ties the existing safety
documents into one design story for a kernel engineer evaluating whether
to trust or build on Rux. Nothing here is new policy; every section
synthesizes a document that carries the detail and the artifacts:*

| Topic | Evidence document |
|---|---|
| The six-layer defense model | [security-design.md](security-design.md) |
| The bug taxonomy + honest gaps register | [safety-and-performance.md](safety-and-performance.md) |
| The unsafe census (generated per round) | [../development/tcb-ledger.md](../development/tcb-ledger.md) |
| The ≤5,000-line / 1% ladder + hardening layers | [../development/unsafe-5000-plan.md](../development/unsafe-5000-plan.md) |
| The per-region verification ledger (generated) | [../development/unsafe-verification.md](../development/unsafe-verification.md) |
| The layer map (Framework / Service / diagnostics) | [kernel-layers.md](kernel-layers.md) |
| Lock invariants | [lock-ordering.md](lock-ordering.md) |
| Interface divergences (D-numbers) | [design-divergences.md](design-divergences.md) |
| Rust-native design register (R-entries) | [rust-native-register.md](rust-native-register.md) |

Rux is a monolithic kernel written in Rust with a Linux-compatible ABI.
There is no microkernel message boundary to hide behind, so the memory
safety story is layered inside one address space — and each layer is
chosen so the cheapest mechanism carries the most weight.

---

## 1. The threat model: what memory-safety failure looks like here

In a C kernel the threat model for memory safety is the whole tree:
every function is review-bound, the classic defect classes (buffer
overrun, use-after-free, double-free, wild write, data race on shared
memory) are expressible everywhere, and the runtime tools that catch
them (KASAN, KCSAN, FORTIFY) exist precisely because the language
cannot. The TCB is the entire codebase.

Rux's TCB is defined by the language instead: memory safety of the
non-`unsafe` bulk is compiler-enforced, so the review-bound surface is
the set of lines inside `unsafe` constructs — 14.0% of kernel/src on
the honest (dual) measure today
([../development/tcb-ledger.md](../development/tcb-ledger.md)). The
threat model therefore has two parts: what can still go wrong inside
that surface, and what the residual safe-code risks are.

The empirical answer comes from the bug taxonomy
([safety-and-performance.md §2.1](safety-and-performance.md)): a sweep
of the entire repository history — 2,702 kernel-code commit subjects
deduplicated from 3,358 — with every corruption-vocabulary candidate
individually adjudicated and every memory-corruption fix diff inspected
for where the defect lived. The result:

**33 memory-corruption-class bugs, with residency:**

- **23 had the defect inside an explicit `unsafe` block, arch
  assembly, or an unsafe interior** (raw `*mut Task` protocols,
  `UnsafeCell` allocator metadata). Examples that show the flavor: a
  `write_bytes` element-count overrun that zeroed 28 KB past a
  pt_regs frame; x86 entry leaving EFLAGS.DF dirty so user-set DF
  reversed `rep movsb` and kernel copies wrote below the destination;
  the buddy allocator handing the same block to two owners after a
  split left it unmarked; page-table teardown freeing frames another
  address space still linked.
- **10 had the defect in safe code** — and every one of the ten is an
  ownership/refcount/lock-protocol/stack-depth decision that safe Rust
  does not type-check today (a refcount-underflow arm freeing a page
  another CPU already freed; an RAII guard dropped on an early-return
  path freeing live page tables; a discarded `#[must_use]` guard
  releasing a lock at end of statement; a check-then-write on an
  unlocked field). The corruption executed through a trusted unsafe
  boundary or an unguarded stack.
- **Zero pointer-expression defects ever appeared in safe code.** No
  indexing overrun, iterator invalidation, wild pointer arithmetic, or
  format-string injection — the classes that dominate C-kernel CVE
  history are compile errors in the safe surface. The one would-be
  out-of-bounds WRITE in safe code was stopped by a bounds check and
  surfaced as a loud panic.
- **Six of the 33 were caught by review before any boot ran them.**

The classes that are structurally absent from safe code: buffer
overruns (bounds-checked slices), use-after-free/double-free of
`Arc`-owned objects, data races on typed shared state (`Send`/`Sync`),
iterator invalidation (borrowck), format-string injection. The
residual safe-code risk concentrates exactly where ownership is
manual — physical frames, page tables, cross-task lifetimes — and
that is what the typed-handle work (§3) and the lock protocol (§4)
attack.

One deliberate classification rule keeps the record honest:
containment-only commits (heals, guards, tripwires) are not counted as
bugs, instrument misdiagnoses are recorded alongside real finds, and
negative results stay archived with receipts
([safety-and-performance.md §2.1](safety-and-performance.md)).

## 2. The design in one diagram

The layers stack so each one backs the layer above it: the language
removes the defect classes; typed primitives shrink what `unsafe`
remains to single audited edges; the territory boundary makes the
shrunk surface structural (compiler-enforced); the lock protocol
removes the concurrency failure shapes inside the TCB; hardware
converts residual corruption classes into guaranteed faults; formal
verification checks what humans argue; the runtime backstop makes the
surviving failure loud instead of silent.

```
                untrusted user memory
                        │
  ┌────────────────────────────────────────────────────────────┐
  │ L2+L4  syscall boundary — user-memory access only through   │
  │ UserPtr/UserSlice (access_ok + exception-table copies);     │
  │ SMAP armed, AC-bracketed usercopy (~55 ns/bracket)          │
  └────────────────────────────────────────────────────────────┘
                        │  typed calls only, no raw pointers
  ┌────────────────────────────────────────────────────────────┐
  │ L1  service layer — fs, net, ipc, drivers, syscall glue:    │
  │ safe Rust; #![forbid(unsafe_code)] spreading file-by-file   │
  │ to crate-wide (the territory boundary, §3)                  │
  └────────────────────────────────────────────────────────────┘
                        │  INV-contract primitives
  ┌────────────────────────────────────────────────────────────┐
  │ typed primitives — kernel/src/safety/, arch::uaccess_safe,  │
  │ process::task_ref: KPtr / StaticRef / DmaBlock / DmaSlot /  │
  │ MmioRegion / UserPtr / TaskRef / safe_call — one audited    │
  │ unsafe edge per invariant, safe API above                   │
  └────────────────────────────────────────────────────────────┘
                        │  inside the Framework (the TCB)
  ┌────────────────────────────────────────────────────────────┐
  │ lock protocol — owner-in-word, single ordered store,        │
  │ typed lock keys (D-11), INV-LOCK-1..8; the concurrency      │
  │ failure shapes removed at the protocol level (§4)           │
  └────────────────────────────────────────────────────────────┘
                        │  hardware backstop
        SMEP armed · page-0 refused at every table walk ·
        CR0.WP from earliest boot · no identity/physical window
                        │  verification + runtime backstop
        Kani / loom / SPIN / proptests pinned by mirror-sync ·
        Miri on the verify crate · faults land loud (panic, not
        silent corruption)
```

The ordering principle is the one [security-design.md](security-design.md)
states: **type system first, hardware second, verification third,
runtime mitigation last** — the cheapest mechanism carries the most
weight, and nothing on the fast path pays for a check the compiler
already discharged. That is also why safety and speed are not in
trade here: the measured fast paths run on the same typed structures
the safety argument reasons over
([safety-and-performance.md §3](safety-and-performance.md)).

The six defense layers of [security-design.md](security-design.md)
(L1 language, L2 syscall boundary, L3 privilege model, L4 hardware
aids, L5 formal verification, L6 runtime mitigation) map onto this
diagram; this document covers the memory-safety spine (L1, L2, L4,
L5), with L3 (the capability model, seccomp, the LSM hook point) and
L6 (planned eBPF-class mitigation channel) specified there.

## 3. The safety primitives: typed handles with INV contracts

The unsafe mass is not spread evenly. The round-1/2 census histogram
showed it concentrated in mechanical families — raw-pointer
dereferences around long-lived allocations (36% tree-wide) and
per-call-site `unsafe { unsafe_fn() }` wrappers (15%). The answer is a
shared primitive layer whose wrappers carry the safety proof in the
type: **construction is the one audited unsafe edge, every later
access is safe code.**

The catalogue lives in `kernel/src/safety/` (`ptr.rs`, `dma.rs`,
`mmio.rs`, `call.rs`), plus two earlier typed families:
`arch/uaccess_safe.rs` (user memory) and `process/task_ref.rs` (task
handles). Each primitive documents its invariants as numbered
INV- contracts, and each unsafe block inside it is the single audited
edge for one invariant:

| Primitive | Family it retires | Contract |
|---|---|---|
| `KPtr<T>` — owned kernel allocation | raw `*mut T` + hand-written `dealloc` per exit path (the layout-drift and double-free classes) | INV-KPTR-1..5: one audited alloc edge; unique ownership → safe `Deref`; `Drop` deallocates with the same `Layout::new::<T>()` so the layout cannot drift; named escape hatches for DMA; device-written reads via `read_volatile` |
| `StaticRef<T>` — machine-lifetime shared reference | static raw-pointer hand-outs (the gen_disk class) | INV-SREF-1..3: storage outlives the machine and each site names it; shared views only; no offset arithmetic inside the type |
| `DmaBlock` / `DmaSlot` — device-shared carve | hand arithmetic over one alloc split into virtqueue rings | INV-DMA-1..3: one tracked allocation, layout-stored `Drop`; `carve` checks alignment, bounds, disjointness (append-only frontier); device-written fields only through bounds-checked volatile paths |
| `MmioRegion` — device-register window | 151 hand-computed `(base + off) as *mut u32` volatile sites in mm+drivers | INV-MMIO-1..3: mapping proven at probe; every accessor bounds-checked (out-of-range panics loudly instead of walking foreign memory); all accesses volatile |
| `UserPtr<T>` / `UserSlice` — user-memory entries | per-site `unsafe { copy_to_user }` with caller-obligation validity | INV-UACCESS-1: kernel side proven by the type system (`&[u8]`/`T: Copy`), user side access_ok-checked and faulting through the exception table — ANY address value is sound to pass, a bad one reports uncopied bytes; handles `!Send`/`!Sync` |
| `TaskRef` / `TaskBorrow` — task handles | raw `*mut Task` protocols in pid_hash/runqueues | pin owned by the handle (`Drop` = `task_put`, no leak path); borrow valid for syscall-context short reads only, not across yield points |
| `safe_call!` / `safety_cast` — call discipline | per-call-site restated (or silently trusted) preconditions | INV-SAFECALL-1..3: one safe wrapper per dischargeable precondition, signature makes violations impossible or loud, no self-recursion (the round-2 bug class); INV-SAFECAST-1: size-asserted single `transmute_copy` |

Two construction disciplines make the catalogue trustworthy:

1. **One audited alloc, narrow edges.** Every primitive has exactly
   one unsafe edge per invariant (construction), and everything after
   the edge is safe. The extreme counterexamples are measured: of the
   ~18,648 `unsafe fn` body lines, a handful of functions hold most of
   the mass (`__schedule`, 1,053 lines; `new_task_at`, 766 lines and
   293 raw-pointer writes) — the ladder's first round is exactly
   splitting these into "safe body + narrow audited edge"
   ([../development/unsafe-5000-plan.md](../development/unsafe-5000-plan.md)).
2. **No second authority, no heals.** A primitive (or lock, or
   tracker) must never hold two representations of the same fact that
   can disagree, and a recovery path must never act on absence of
   evidence. The GRQ campaign convicted both shapes (§4); the
   primitives are built so the word/owner/state is the single
   authority, and detection surfaces facts instead of repairing them.

Primitives never grow domain logic — they hold the pointer discipline;
device protocol stays in drivers. Escape hatches (`as_ptr`,
`into_raw`, `ptr_at`) exist for the queue/DMA internals that still
live on raw addresses, and their docs name the obligation that
transfers to the caller (the process rule that turns them into
per-site ledger rows is §7).

Related typed designs outside `kernel/src/safety/` carry the same
idea into specific hot paths: typed per-CPU storage whose guard
lifetime is the IRQ-off window (D-7), typed lock keys (D-11), and the
typed timer wheel whose six misuse classes are compile errors (D-19)
— all registered with measured evidence in
[design-divergences.md](design-divergences.md) and
[rust-native-register.md](rust-native-register.md).

## 4. The lock protocol: one authority per lock word

The scheduler/lock campaign (the GRQ record in
[safety-and-performance.md](safety-and-performance.md)) convicted a
set of recurring failure shapes in kernel lock protocols and fixed
them at the protocol level, not one patch per occurrence. The shapes,
and their structural fixes:

1. **Two-step release.** The round-6 GRQ made every release two steps
   (per-cpu tracker clear, then word store); a fault landing in the
   gap stranded a word no tracker claimed — the "seven waiters, no
   holder" incident class. **Fix: owner-in-word.** The owner lives in
   the lock word itself (`0` free, `cpu+1` held-by-cpu): acquire is
   one `cmpxchg`, release is **a single ordered Release store of 0**
   — `spin_unlock` stays the reference kernel's single ordered store,
   and there is no gap for a fault to land in. The tracker, the
   heal, and the orphan-word triage branch are all deleted
   (d56e5c8ad); `grq_held_here` and the terminal release's ownership
   proof are derived from the word — one authority that cannot
   disagree with itself.
2. **Second-authority trackers.** Any held/owner side-table that
   gates a release can drift from the word it mirrors. **Fix:** the
   only authorities are the word and the reference kernel's own owner
   fields (rwsem/mutex semantics); debug ownership checks exist as
   `debug_assert`s (the `CONFIG_DEBUG_SPINLOCK` analog), compiled out
   of release behavior.
3. **Release without positive knowledge.** The exit-path stack-free
   family freed a stack its owner still executed on because "no
   evidence of use" was treated as "safe to free". **Fix: positive
   knowledge before a free** — `Task.terminal_switch_done` is set
   only at the real-switch retire sites (aborts never set it), and
   every stack-free predicate requires it; recovery paths **defer or
   withhold** instead of proceeding (a heal-fail re-parks the member;
   a stale-rsp0 stack is leaked, never re-carved).
4. **IRQs-off-window deliveries.** Wakes issued while holding a lock
   whose section runs with IRQs off turn the wake into unbounded work
   inside the window. **Fix: window-then-deliver** — collect under
   the lock, deliver after release (binder's deferred wakes, futex's
   proof-carrying wake-under-bucket being the documented bounded
   exception, INV-LOCK-5).
5. **Heal-style remediation.** A self-invented "heal" that
   force-released an unclaimed word acted on absence of evidence —
   the reference kernel has no counterpart, and the honest reading
   was that the *protocol* was wrong. **Fix: the source-first
   doctrine.** No heals: when a detector fires, fix the source
   protocol; detectors may report, never repair.
6. **Print-path blindness.** Forensic printers inside raw-visible
   deadlock/death reports were no-ops in release builds (`pr_debug`,
   `putchar_no_lock`) — six such blind printers were found across the
   campaign and unblinded to the lock-free raw console; reports print
   release-first and rate-limited, and hot-path sections carry
   assertions instead of prints (§7).

The sweep methodology extends the campaign: every static lock in the
tree is audited against these classes (147 declarations across 81
files at sweep time), with the D-11 typed-lock-key table as the map
and [lock-ordering.md](lock-ordering.md)'s INV-LOCK-1..8 as the
binding nesting invariants. The GRQ itself is the largest single
structure — it is also the one the per-boot scheduling census
adjudicates on every quiet boot (TERM-DESCHED 8±2 / WAKE-REFUSED
32±4), which is what made the campaign's evidence collection possible
in the first place.

## 5. The measurement: two numbers, banked every round

The census is mechanical and banked per campaign round
([../development/tcb-ledger.md](../development/tcb-ledger.md),
generated by `scripts/tcb_ledger.py`):

**The two numbers (2026-11-20 baseline, 361,875 kernel/src lines):**

| Measure | Lines | Share |
|---|---:|---|
| explicit `unsafe { … }` block bodies | 25,769 | **7.1%** |
| `unsafe fn / impl / extern` bodies (780 items) | 27,097 | 7.5% |
| union ("dual" — the honest total) | **50,630** | **14.0%** |

**The honest history.** The first headline (12.9%) counted only
explicit blocks on the early tree; the first re-bank on the grown tree
read 10.6% block-only (44,337 of 417,023 lines). Both numbers were
incomplete: `unsafe fn` bodies were invisible to the block-only scan,
and a stale 27,625-line generated file was inflating the denominator
(silently diluting every percentage). Two corrections landed on
2026-11-19/20: the **dual metric** (fn/impl bodies counted, union
reported) and the **scope rule** (git-tracked sources only). The
pre-campaign dual union was 19.6%; the campaign's reduction rounds
have since moved the block share 12.1 → 10.9 → 9.5 → **7.1%** (ipc
alone went 24.3% → 1.4% with the binder conversion). The correction
is recorded, not buried — the block-only series is kept for trend
continuity.

**The ladder to ≤5,000 lines / ≤1%**
([../development/unsafe-5000-plan.md](../development/unsafe-5000-plan.md)):
the two numbers shrink by different work, and the end state makes the
claim auditable rather than aspirational:

| Milestone | Union share | Work |
|---|---|---|
| Now (2026-11-20) | 14.0% | — |
| Round A: the 36 big unsafe fns split into safe body + narrow edge | ~10–11% | converts ~8,300 fn-body lines to compiler-checked code |
| Round B: typed handles on the worst raw-pointer writers | ~8–9% | `new_task_at`'s 293 raw writes become TaskRef/KPtr-class |
| Round C: the ~470 small unsafe fns primitive-ized or un-signed | ≤2.5% | move under INV contracts, or convert via an existing primitive |
| Round D: per-block irreducibility proofs | **≤1% (≤5,000 lines)** | which primitive owns each block, why no safe shape exists, the reference-kernel counterpart — ledger-verified |

**The per-region gate.** Beyond the line census,
`scripts/unsafe_gate.py` enforces that every unsafe region in the tree
is accounted for in the verification ledger
([../development/unsafe-verification.md](../development/unsafe-verification.md),
generated by `scripts/unsafe_census.py --ledger`): 5,841 census
regions today, each tagged by category (ptr-ops, volatile, asm,
addr-arith, …) and carrying either a named artifact (46 so far: Kani
harness, proptest, loom model, Miri mirror, or INV id) or an explicit
GAP — accounted, unverified, visible. The token metric is tracked
alongside: 3,166 of 5,224 unsafe constructs sit outside the Framework
layer, SAFETY-contract coverage 3,261/5,210 (63%).

**The rule:** the whole-kernel percentage may only go down, or an
increase is justified per release; arch bring-up raises it
temporarily and must shrink back; unsafe added by any fix requires a
commit-body sentence saying why safe code was not possible.

## 6. The verification stack: who checks what

Four verification regimes cover the shrinking unsafe surface, ordered
by who does the checking:

**Human-argued.** Every unsafe edge carries its invariant as a SAFETY
comment or an INV row — the numbered contracts of §3 and §4, the
INV-LOCK-1..8 nesting table
([lock-ordering.md](lock-ordering.md)), the per-region SAFETY
coverage (63%). The audit is the record: the taxonomy sweep reads
these rows, and a missing row is a visible hole, not an implicit
assumption.

**Machine-checked.** The verify crate mirrors the kernel's algorithms
and proves them, pinned to the code it covers by the mirror-sync
checker (`scripts/verify_sync_check.py` — a drifted proof fails the
build, so a proof cannot silently stop covering its code):

- **Kani** bounded model checking: 164 `#[kani::proof]` harnesses,
  proving core unsafe routines for ALL inputs.
- **loom** interleaving enumeration of the sync primitives: 8
  scenarios (3 planted must-catch); it caught a real kernel
  lock-protocol defect 90 seconds after first arming (011a71c5, a
  mutual-exclusion break).
- **SPIN** models of the lock/consistency protocols: 4 models, 8 LTL
  properties, all PASS.
- **proptest** rules over scoring/encoding/algebra surfaces: 1,184
  host tests in the verify crate.

**Runtime-checked.** Miri executes the verify mirror and is the
ground truth for undefined behavior — anything Kani or a human misses
it catches at run. The expansion in flight runs the safety
primitives' full test battery under Miri in CI (the "geology check":
a UB hit below the INV contracts invalidates every SAFETY argument
built on it, so it is treated as P0-below-the-P0s). Planned next: Kani
bounded proofs for the two riskiest invariant families — KPtr's
exactly-once free and MmioRegion's no-OOB-window — and one Verus
pilot (MmioRegion's bounds invariant) to decide whether full
deduction pays on the shrunk TCB
([../development/unsafe-5000-plan.md](../development/unsafe-5000-plan.md) Layer 2).

**Hardware-backed.** The scribbler campaign's facts, now standing
invariants: **SMEP armed** (CPUID.7.0:EBX[7]-gated, per-CPU, BSP+APs)
so a kernel rip landing on user-mapped or zero pages faults at the
fetch instead of executing garbage; **page-0 refused at every table
walk**; **SMAP** with AC-bracketed usercopy (D-18; the capability bit
is read from the correct CPUID register and the boot log asserts the
armed state); **CR0.WP** set from earliest boot so supervisor stores
cannot ignore page-table RW bits; **no identity/physical-memory
window** (D-17) so a bad user pointer faults instead of returning
kernel bytes. Each converts a corruption class into a guaranteed,
loud fault. Planned: the MPK evaluation — primitive-internal metadata
pages under a protection key with ERIM-class switching (~1% class
cost), gated like all opt-in hardware (off by default, verified by
construction), with MTE documented as platform-bound, not pursued.

The runtime backstop principle underneath all four: a surviving
defect lands as a caught, loud panic — the panic surfaces are a
designed, enumerated set with regression receipts
([safety-and-performance.md §2.2](safety-and-performance.md)), not a
latent class.

## 7. The process rules

The measurement and verification stack is held up by six standing
rules, each adopted from a convicted failure shape:

1. **Soundness boundary = module boundary.** Every crate exposing a
   safe API owns its soundness end-to-end; `as_raw()`/`from_raw()`
   escape hatches are private by default, and the CI gate fails any
   public escape hatch without a per-site ledger row.
2. **No heals (source-first doctrine).** A detector reports; it never
   repairs. Recovery paths defer or withhold on missing positive
   knowledge; acting on absence of evidence is the convicted mistake
   shape (§4.3, §4.5). When a detector fires, the fix goes into the
   source protocol.
3. **Hot paths carry no prints.** In-section prints become
   assertions; forensic reporting moves to the lock-free raw console,
   release-first and rate-limited. printk itself is safe-and-non-
   sleeping (lock-free ring, deferred wake), so the debug machinery
   cannot wedge the lock protocol it is reporting on.
4. **Release builds ship with debug features off.** Detectors are
   `debug_assert`s or release-profile-qualified; the disabled path
   compiles to a load+test+branch (verified in release disassembly
   for the tracepoints; the same discipline governs the safety
   detectors), so debug checking costs nothing when disabled and
   cannot silently change release semantics.
5. **Third recurrence triggers a primitive-design review.** Instead
   of fixing unsafe expressions one by one, the recurring failure
   shapes are classified in the INV ledger; the GRQ campaign's
   classes each got a protocol-level fix, and the standing rule is
   that every third recurrence of the same INV family reviews the
   abstraction's shape rather than patching a fourth occurrence.
   `scripts/unsafe_census.py` grows a pattern-histogram column per
   release so shape drift is visible in CI.
6. **The census is banked, and it only goes down.** Every reduction
   round re-banks the census; unsafe added by a fix needs a
   commit-body justification; the merge gate for a safety-track
   change reports its layer's metric before and after
   ([security-design.md §6](security-design.md)).

## 8. The honest gaps: what remains

The claim this document supports is bounded, and the bounds are
registered:

- **The pt_regs writer hunt is open.** The exit-path strand family's
  smallest face — a writer that lands small-int values
  (rip=1/rsp=0/badaddr=3) into pointer fields — remains at large and
  seeds every downstream strand; the SMEP/page-0 closures bound what
  it can amplify, but the producer is not yet convicted
  ([safety-and-performance.md](safety-and-performance.md), the
  oh-launch rounds; the hunt method is the scribbler campaign's,
  [../archive/r74-scribble-hunt.md](../archive/r74-scribble-hunt.md)).
- **The irreducibility proofs are pending.** The ≤1% end state
  depends on Round D's per-block proofs (which primitive owns each
  surviving block, why no safe shape exists, the reference
  counterpart). Until they land, ≤5,000 lines is a plan with
  milestones, not a banked fact
  ([../development/unsafe-5000-plan.md](../development/unsafe-5000-plan.md)).
- **Escape hatches are not yet CI-enforced.** The primitives'
  `as_ptr`/`into_raw`/`ptr_at` hatches carry doc-stated obligations,
  but the rule "no public escape hatch without a per-site ledger row"
  is adopted, not yet implemented in `unsafe_gate` (layer 4.1 of the
  plan).
- **The MPK evaluation has not run.** The host is x86_64 KVM and PKU
  should be exposed to the guest; the check (CPUID.7.0:ECX[3,4]),
  the ERIM-class design, and the fault-injection acceptance probe
  are specified. Until it runs, the unsafe interior has no
  hardware-memory-key discipline — a wild write inside the Framework
  can reach anything the address space covers.
- **The safe-code ownership residuals** the taxonomy names: refcounts
  are plain atomics (two of the ten safe-class bugs are exactly the
  class a checked Refcount newtype catches); the data-frame free vs
  TLB-shootdown-completion ordering for multi-threaded mms is
  documented with its closing shape (the reference's mmu_gather
  analog); overflow checks are off in the release profile (one line
  plus a measured gate cost, pending).
- **The C-kernel hardening analogs not yet present**, stated plainly
  in the gaps register
  ([safety-and-performance.md §5](safety-and-performance.md)): KASLR
  has plumbing but no entropy source; kernel text/rodata W^X is not
  enforced (no mark_rodata_ro pass); no compiler stack-protector;
  hardened usercopy is access_ok only (no per-object region
  whitelist); no KCSAN-class watchpoint checker; the LSM hook point
  exists but policy enforcement is permissive by construction.

Each gap entry in the register names what closing looks like and who
owns it; the register only shrinks by evidence, and every round that
closes one appends the closure to the same document.
