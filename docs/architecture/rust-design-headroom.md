# Rust Design Headroom — where the kernel can be structurally better

Companion to `design-divergences.md`: that register records KEPT
divergences; this document analyzes where the language model lets us design
subsystems structurally better than the mainstream C-kernel approach —
turning runtime bug classes into compile-time impossibilities. Every item
obeys the three gates (external compatibility · LTP green · no upper-layer
breakage) and the perf merge gate.

The analysis is anchored in bug classes this repo actually paid for during
the OH campaign — each section names the real incident.

## 1. File operations as capability traits (fs/)

**Mainstream approach:** a struct of function pointers per file type, many
left NULL; every call site must null-check, and the wrong errno on a
missing handler is a bug class. **Paid incident:** read() on a valid epoll
fd returned EBADF instead of EINVAL because the read slot was NULL — an OH
event loop wedged spinning 155,991 times before the semantics were fixed.

**Better design:** model each operation as a trait (`Readable`, `Seekable`,
`Pollable`, `Mappable`) with total functions; file types implement exactly
the capabilities they have. The dispatch becomes a match on capability
proofs — a missing capability is a *type-level* fact, and the errno mapping
lives in one place instead of per-callsite conventions. Extend the same
idea to io_uring-style batched op sets later.

## 2. Ownership instead of lifetime discipline (mm/, fs/, ipc/)

**Mainstream approach:** refcounted objects + manual lifetime rules;
use-after-free and double-free are runtime phenomena found by sanitizers —
or by a two-month capture campaign. **Paid incident:** the entire wildwrite
hunt (r1–r8x rounds, zero captures under instrumentation, but the original
corruption that motivated it) is this class.

**Better design:** `Arc`/`Weak` plus channel-based ownership *transfer* for
cross-CPU handoffs (a work item sent to another core is moved, not shared);
intrusive structures (`ArcListNode`) so list membership is part of the
object's type. Target: shrink the guarded capture surface as ownership
proofs replace runtime watchpoints.

## 3. Typed per-CPU state and lock-free queues (sched/, mm/)

**Mainstream approach:** per-CPU structs accessed by raw offset with
implicit preemption-safety conventions. **Paid incident:** allocator
per-CPU magazines existed as a bolt-on fix; the pre-magazine global lock
was 12.6% of the machine.

**Better design:** const-generic per-CPU storage with typed handles that
*encode* the interrupt-disabled context in the type (a handle only exists
inside a `with_cpu_local` closure) — the irqsave discipline becomes
type-enforced rather than review-enforced. Same for the run queue: the
existing GRQ (divergence D-1) can gain a typed lock key so lock-order
violations (INV-LOCK-*) are rejected at compile time via lock-key types.

## 4. Deriving interface formats from structure definitions (procfs/, dfx/)

**Mainstream approach:** hand-written format strings per file; drift is a
review problem. **Paid incident:** five byte-format defects (column order,
trailing spaces, timestamps in the wrong unit, prefix pollution, shifted
columns) survived into a merged tree and needed a dedicated review pass to
catch.

**Better design:** one `procfile!` macro (or const table) that derives the
formatter from a field declaration list — field name, width, unit — so the
format and the data stay one artifact; the same macro emits the unit-test
that snapshots the format. New /proc additions become a one-place
declaration, killing the drift class.

## 5. Result-typed syscall boundary (syscall/)

**Mainstream approach:** errno returned as -1 magic values through layers;
sign bugs and errno-mangling (the u32-microsecond wrap that killed an SA
host; the negated-nice encoding bug) recur. **Paid incidents:** both named
above.

**Better design:** internal kernel functions return `Result<T, Err>`
throughout; the errno mapping happens exactly once at the boundary. `#[must_use]`
on hot-path Results turns ignored errors into build failures.

## 6. Binder/IPC transaction objects with move semantics (ipc/)

Transactions are naturally single-ownership messages. Type the transaction
lifecycle (pending → in-flight → completed → reaped) as an enum state
machine; illegal transitions (e.g., replying to a reaped transaction)
become unreachable states instead of runtime corruption. The SLOW-TXN
forensics become state-machine instrumentation for free.

## 7. Timer wheel as typed slot ranges (drivers/timer/)

The hrtimer/wheel split today is a fix-on-fix history (empty-raise gates,
resync thresholds). A typed design: deadline objects carry their own slot
arithmetic and the wheel enforces bucket invariants by construction —
plus const-generic wheel sizes per use (tcp timers vs hrtimers).

## 8. Driver model with capability proofs (drivers/)

virtio device handling can use trait objects whose capabilities (DMA
mapping, interrupt affinity, queue depth) are attached at probe time and
checked by the type system at use — shrinking the runtime validation that
today guards the completion paths (historically the most-reworked code in
the repo).

---

## Priority by expected payoff

1. **§4 procfile macro** — smallest scope, kills a freshly-paid bug class,
   immediate for the DFX lane.
2. **§1 capability traits** — medium scope, kills the errno-class at the
   biggest syscall surface; starts from the EINVAL fix already in flight.
3. **§3 typed per-CPU + lock keys** — direct perf relevance (magazine
   discipline enforced); medium scope.
4. **§2 ownership campaign** — largest scope, retires the guarded capture
   surface over time; start with fs/ page lifetime (Arc-based) where the
   wildwrite campaign already instrumented.
5. **§5/§6/§7/§8** — steady-state adoption as each area is touched.

The performance lane's north-star goal (reference-beating target) is
defined in `docs/development/perf-report.md` §8 and tracked there; this
document is the architectural lever list it draws from.
