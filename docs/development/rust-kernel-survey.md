# Rust Kernel Landscape Survey and x86_64 Refactor Notes

> **Status**: research notes + actionable refactor list for the x86_64 port.
> The x86_64 architecture port is the first structural change since the
> aarch64 removal, and every file it touches is a window to tighten
> conventions. This document has three parts:
>
> - **Part A** — engineering practices we adopt, written as standalone
>   how-to guidance (no attribution to any specific kernel; these are
>   standard techniques, independently implemented).
> - **Part B** — a survey of the Rust-kernel landscape, academic and
>   industrial, covering roughly 2020–2026, with named projects and what
>   each contributes to our decisions.
> - **Part C** — the concrete list of refactors landing during the x86_64
>   port, mapped to real files.

---

## Part A — Adopted engineering practices (how-to only)

### A1. Per-CPU data structure discipline

One `#[repr(C)] #[repr(align(64))]` struct per CPU, collected in a static
array indexed by CPU number — no GS/TP-register cleverness required to
start, and the same shape works on both architectures:

```rust
#[repr(align(64))]
pub struct PerCpu {
    // --- written only by this CPU ---
    pub current: AtomicU64,
    pub in_irq: u32,
    _pad0: [u64; 5],                 // fill out to a cache line
    // --- read/written cross-CPU: own cache line(s) ---
    pub cpu_id: AtomicU32,           // published once at startup
    call_queue: Spinlock<...>,       // cross-core mailbox
    _pad1: [u64; 6],
}
static PER_CPU: [PerCpu; MAX_CPUS] = ...;
```

Rules:

1. **Cache-line partitioning**: fields shared across cores go into their
   own cache lines, separated from CPU-private fields by explicit padding.
   The padding fields are sentinel documentation — a reader sees the
   boundary, not just an align attribute at the top.
2. **Publish your own CPU id** inside the struct (atomic store once at
   startup) so cross-core code can address "the CPU that owns X" without
   a second mapping structure.
3. Cross-CPU access goes through the shared-line fields only; touching
   another CPU's private fields is a bug.
4. The per-CPU struct is the one place the "current CPU number" question
   is answered. Everything else reads from here (directly or via a
   register cached copy later).

Applied in: `kernel/src/arch/x86_64/smp.rs` (new), and the pattern is
what the riscv64 `tp`-based scheme should converge to over time.

### A2. TTAS spinlocks (test-and-test-and-set)

Acquire with CAS; on failure, spin on a plain load with `spin_loop()`:

```rust
while self.lock.compare_exchange(false, true, Acquire, Relaxed).is_err() {
    while self.lock.load(Relaxed) { core::hint::spin_loop(); }
}
```

The failing core stops broadcasting atomic operations while waiting —
only the release re-triggers coherence traffic. Pair with a bounded-spin
panic (debug builds) as cheap deadlock detection, and keep the invariant
"spinlocks are never held across anything slow" enforced by review.

Audit target: `kernel/src/sync/spinlock.rs` — confirm the current
implementation already follows TTAS; if it re-issues CAS in the inner
loop, fix it (one-line change, measurable on contended paths).

### A3. Uniform-signature syscall dispatch table

Replace the large `match nr { ... }` dispatcher with a const table of
uniformly-signed functions:

```rust
type SysFn = fn(&mut SyscallCtx) -> SysRet;   // None = unsupported
static SYSCALL_TABLE: [Option<SysFn>; NR_MAX] = build_table();

const fn build_table() -> [Option<SysFn>; NR_MAX] {
    let mut t = [None; NR_MAX];
    t[SYS_READ] = Some(sys_read);
    ...
    t
}
```

Why: a `match` whose arms call functions with differing signatures forces
the compiler to set up each call at every jump target — two
branch-mispredict-prone transfers per dispatch and duplicated prologues
in the jump table. A function-pointer array is one indirect call with a
single predictable branch. All handlers take one context struct and pull
arguments themselves, which is also closer to how the asm entry will pass
a single frame pointer.

Applied in: `kernel/src/syscall/dispatch.rs` — mechanical migration,
gated by LTP full-sweep parity.

### A4. Two-stage syscall fast path (design note — needs ABI audit first)

Split syscall handling: a **fast path** that preserves callee-saved
registers by construction (a plain Rust function call already does) and
returns directly to user mode when it can finish; a **slow path** that
saves the full trap frame and enters the scheduler when it cannot.

Caveat before implementing on x86_64: the Linux syscall ABI requires the
kernel to preserve every register except `rax` (return), `rcx`, `r11`
(consumed by `syscall`/`sysret`). A fast path written as a normal Rust
call preserves `rbx/rbp/r12–r15` by the ABI, but *clobbers* the
caller-saved argument registers (`rdx, rsi, rdi, r8, r9, r10`) — which
Linux preserves. User-space code generally tolerates clobbered argument
regs, but "generally" is not "verified": audit glibc/musl hot paths (or
save the six regs, still cheaper than a full frame) before enabling.
Expected win is on the order of tens of cycles per trivial syscall
(getpid/gettid/…); worth it only after Ubuntu boots, measured with the
existing LTP/perf harnesses.

### A5. Wake-ordering discipline in sleep/wake primitives

For any "sleep until flag" primitive, the release sequence must be:
**(1) extract the waker, (2) set the runnable flag (release), (3) invoke
the waker**. Reordering (2) before (1) risks use-after-free of the
waiter; reordering (3) before (2) risks a lost wakeup: the woken task
polls, observes the flag still clear, sleeps again, and the subsequent
flag store has no one to wake. Write this invariant as a comment at
every wake site — it is the class of bug that only appears under load.

Audit target: `kernel/src/sync/condvar.rs`, `sync/futex.rs` wake paths.

### A6. Fork/exec performance family (Phase 4 experiments, measured)

- **Batch page-table updates**: when duplicating or remapping on behalf
  of fork, coalesce TLB flushes and intermediate-level writes per range
  instead of per PTE.
- **Parallel cross-core copy**: split large page copies (fork data,
  exec segments) across CPUs with a work-stealing handoff.
- **Fault prediction**: after fork, prefault the pages the parent
  touched most recently (cheap LRU signal) before the child faults.
- **Zero-copy file cache**: page-cache pages mapped directly into user
  page tables for read-only file segments, no copy into anon memory.

Each of these is a hypothesis, not a commitment; every one needs a
before/after number on the LTP/Ubuntu boot pipeline.

### A7. Documentation culture

Per-subsystem optimization notes carry: the idea, the measured
before/after, and the **negative results** (things tried that did not
help, with the reason). A negative result recorded is a repeated mistake
prevented. The existing `docs/progress/changelog.md` style already does
this for fixes; extend it to performance experiments.

---

## Part B — Rust kernel landscape, 2020–2026

The mission closest to ours is **Asterinas**: a Linux-ABI-compatible
Rust kernel ("framekernel" — one address space like a monolith, but a
privileged framework / unprivileged services split enforced by safe
Rust). At USENIX ATC 2025 it reported 210+ Linux system calls, Linux-par
performance, and a memory-safety TCB of ~14% of the codebase, built on
their **OSTD** kernel-development framework (safe-Rust thread and CPU
context management, safe user-memory access, deferred work). Their
second ATC'25 paper (**Converos**) applies model checking to kernel
concurrency. What we take: the *discipline patterns* — RAII guards for
preemption/interrupt state, CpuLocal as the per-CPU data primitive,
user-memory access that returns `Result` instead of faulting, and
deferred-work APIs with an explicit context. We are not adopting the
framework itself (their architecture differs), but every new x86_64 file
is written to those conventions. Sources:
[The Framekernel Architecture](https://asterinas.github.io/book/kernel/the-framekernel-architecture.html),
[ATC'25 paper PDF](https://www.usenix.org/system/files/atc25-peng-yuke.pdf),
[arXiv:2506.03876](https://arxiv.org/abs/2506.03876),
[LWN coverage](https://lwn.net/Articles/1022920/).

**Theseus** (Rice University, CAPRA group) is the main academic
contribution to *Rust-native OS structure*: "intralingual design" —
reifying OS mechanisms as safe-language constructs instead of external
hooks — and rigorous "state spill" accounting (what state leaks outside
an abstraction). PLDI 2020: "Theseus: an Experiment in Operating System
Structure and State Spill Management". The transferable idea for us is
**invariants expressed in types**: distinct newtypes for physical vs
virtual addresses (we have this — enforce it everywhere), level-indexed
page-table access that cannot decode a level as the wrong one, and
type-state for driver lifecycle. Not transferable wholesale: Theseus is
a single-address-space research OS with live-evolution goals foreign to
a Linux-compatible kernel. Source: [theseus-os.com](https://www.theseus-os.com).

**Rust for Linux** (upstream since 6.1, 2022; drivers and abstractions
since 6.2+; GPU work ongoing through 2025) is the industrial reference
for kernel-side Rust discipline under a C ABI world. Directly usable
patterns: kernel `Error`/`Result` discipline with errno conversion only
at the syscall edge; `Pin`-based self-referential driver objects; guard
types for irqsave/preempt; documenting the safety contract on every
`unsafe` block (our codebase already follows this — keep it a review
gate). Their experience with abstraction maintenance across kernel
releases also validates our per-arch interface approach.

**Hubris** (Oxide Computer, 2022–) demonstrates reliability-first kernel
design: no unbounded/dynamic allocation in steady state, message-passing
tasks with supervisor restart, and *lease-based* peripheral access where
borrowing a device has a scoped lifetime. We are not a microkernel, but
two ideas transfer: (1) interrupt and panic paths must not allocate —
worth auditing our trap/dfx paths for; (2) driver lifetimes read better
when expressed as borrows with scopes, not init/exit flag pairs.

**Redox OS** (2015–, the longest-running pure-Rust kernel) proves the
ecosystem can sustain a full kernel; the pieces worth copying are
process, not architecture: the *book*-style documentation tree and the
RFC mechanism for design decisions — both mirrored (cheaply) by our
`docs/` layout and decision records in port plans.

**Hermit-rs** (Rust unikernel, 2020–) has the cleanest cargo-only kernel
build: custom target JSONs, no out-of-tree scripting for the common path.
With two architectures we now need the same: `make build` and `make
build-x86` both pure cargo invocations differing only in
`--target`/`--features`, plus a CI matrix that keeps both green.

**Tock OS** (embedded, academic-industrial) contributes the concept of
*typed syscall handling* and components whose sharing is impossible by
construction (capabilities as unconstructible-in-user-space types). For
us this mostly reinforces A-part practices: make the illegal states
unrepresentable at module boundaries (e.g. page tables only reachable
through the walker API).

**Verified-kernel line (Verus and friends), 2022–2025**: "Verified
Paging for x86-64 in Rust" (Brun, ETH Zurich, 2022) verified exactly the
component I am writing now — a 4-level x86-64 page-table library in
Rust; **Atmosphere** (2023) pushed verified Rust kernels toward
practicality; Dai et al. (2024) verified page-table implementations for
a Rust TEE with a layered MIR-level framework; Converos (ATC'25, with
Asterinas) targets kernel concurrency. Practical consequence for Rux:
write the new `arch/x86_64/mm/pagetable.rs` as a **typed, level-indexed
walker with pure PTE encode/decode functions** — the same shape our
existing Kani harnesses attach to on riscv64 — so a verification pass
later is incremental work, not a rewrite.

**Stackless-async kernels** (multiple teaching/competition kernels
2021–2023, and the embedded **embassy** executor lineage): interesting,
thoroughly documented, and **not adopted**: measured evidence in that
literature shows per-`await` register spills hurt syscall-heavy paths
especially on register-rich ISAs, and compile times explode. Rux stays
stackful; the two-stage fast path (A4) captures the useful part of the
idea without the executor.

**Industry context** (why this is a safe direction): Rust entered the
Linux kernel (6.1, 2022) and Windows kernel components; Google reported
the memory-safety bug share of new Android code dropping from ~76%
(2019) to ~35% (2022) as Rust adoption grew; US ONCD/CISA reports
(2024) pushed memory-safe languages for critical infrastructure. The
ecosystem bet is settled; our job is the engineering.

---

## Part C — Concrete refactors landing with the x86_64 port

| # | Change | Files | Gate |
|---|--------|-------|------|
| C1 | Arch interface layer (no backend refs outside `arch/`) | `arch/mod.rs` + 93 files | **landed 2026-10-06**; riscv64 build+boot verified |
| C2 | Per-CPU struct per A1 (align-64, cache-line partitioned, published cpu id) | `arch/x86_64/smp.rs` (new); later riscv64 `smp.rs` | x86 boot uses it for current-task/idle |
| C3 | RAII guards for IRQ state (`IrqGuard` with `Drop`), bool fns kept for asm callers | `arch/*/cpu.rs` + gradual call-site migration | new code uses guards; no mass churn of old sites |
| C4 | Syscall dispatch table per A3 | `syscall/dispatch.rs` | LTP full-sweep parity before/after |
| C5 | Typed level-indexed page-table walker, pure PTE codec | `arch/x86_64/mm/pagetable.rs`, `mm_ops.rs` | Kani harness parity with riscv64 suite |
| C6 | Dual-target build: `make build` (riscv64) + `make build-x86` (x86_64), cargo-only | `Makefile`, `build/Makefile`, `build.rs`, `.cargo/config.toml` | both targets green in CI matrix |
| C7 | TTAS audit + bounded-spin debug panic | `sync/spinlock.rs` | microbench on 4-CPU SMP guest |
| C8 | Wake-ordering audit (A5) with invariant comments | `sync/condvar.rs`, `sync/futex.rs` | code review + existing SPIN models extended |

Deferred (post-Uuntu-gate): A4 fast-path syscalls (ABI audit first),
A6 fork family (each behind a measurement), full `Result`-ification of
internal APIs (new code only — no retro-churn).

Sources: [Asterinas book](https://asterinas.github.io/book/kernel/the-framekernel-architecture.html),
[Asterinas ATC'25 PDF](https://www.usenix.org/system/files/atc25-peng-yuke.pdf),
[arXiv:2506.03876](https://arxiv.org/abs/2506.03876),
[LWN: Asterinas](https://lwn.net/Articles/1022920/),
[Theseus book](https://www.theseus-os.com),
[ETH Research Collection (Brun 2022, Verified Paging for x86-64 in Rust)](https://www.research-collection.ethz.ch).
