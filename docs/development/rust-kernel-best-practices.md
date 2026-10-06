# Rust Kernel Best Practices — Whole-Project Refactor Guide

> **Status**: adopted as project-wide engineering standard. This guide
> applies to the **entire kernel tree**, not only the new x86_64
> architecture files. Adoption is staged and gated (§0) — there is no
> big-bang rewrite; every step lands with LTP parity and boot-gate
> verification.
>
> Companion documents:
> - `rust-kernel-survey.md` — the landscape
>   survey (Asterinas, Theseus, Rust-for-Linux, Hubris, ...) that
>   motivates several items here
> - `openharmony-port-plan.md` — the x86_64 port driving the
>   current refactor window

---

## 0. Rules of engagement (how this guide gets applied)

1. **New code follows this guide from the first commit.** The x86_64
   tree, the binder driver, and every new subsystem are reference
   implementations of these practices.
2. **Existing code migrates opportunistically.** A module touched for
   any reason (bug fix, feature, port) is brought up to standard in the
   same change set — except the six *funded* refactors of §9, which are
   scheduled work with their own gates.
3. **Every migration is gated**: riscv64 + x86_64 builds green, LTP
   full-sweep parity within the agreed delta, Ubuntu GUI gate green
   (once x86 reaches it), unit tests green.
4. **The guide outranks local idiom.** Where a module being touched
   conflicts with this guide, the guide wins in that module from that
   change on.

---

## 1. Type-level design — make illegal states unrepresentable

The core Rust advantage in a kernel is not "no null pointers"; it is
that **correctness conditions move from review comments into types**.
A type system check runs on every edit forever; a comment runs once.

### 1.1 Newtypes for every distinct meaning

Every quantity with its own domain gets a newtype, never a raw
`u64`/`usize`:

```rust
pub struct PhysAddr(pub u64);
pub struct VirtAddr(pub u64);
pub struct Pfn(pub u64);          // page frame number
pub struct Cycles(pub u64);       // raw counter ticks
pub struct Nanos(pub u64);        // wall-clock nanoseconds
```

We already have `PhysAddr`/`VirtAddr` in the arch layer — the rule is
that **generic code may not pass raw address integers**. Audit targets:
`syscall/`, `fs/`, `drivers/` (grep `as usize` / `as u64` around
address arithmetic). Conversions between domains (`Pfn::from_addr`,
`VirtAddr::from_pfn`) live as methods on the types, so each conversion
site is findable and reviewable.

### 1.2 Enums over magic numbers

Kernel state machines become enums: TCP already does this; page state
(`PageFlag` bitfields → states where applicable), IRQ line states,
mount states should follow. Pattern matching then makes forgotten
states a compile error instead of a latent bug.

### 1.3 Type-state for lifecycle

Objects whose methods are only valid in certain phases encode the
phase in the type. Two concrete applications in Rux:

- **Page tables are reachable only through the walker API.** Raw
  `*mut PageTable` exists inside `arch/*/mm/` and nowhere else; generic
  mm code manipulates mappings through `PageTableWalker`.
- **Devices**: `Probed<S>` → `Initialized<S>` → `Running<S>` (or the
  zero-cost equivalent: methods that consume/produce capability tokens,
  §1.4). Avoids the init-flag-pair pattern where a not-yet-probed
  device is callable.

### 1.4 Capability tokens for privileged operations

Operations that require *authority*, not just data, take a token that
only the legitimate owner can construct:

```rust
pub struct IrqLine { irq: u32 }        // authority to manage one line
impl IrqChip {
    pub fn line(&mut self, irq: u32) -> IrqLine;   // the ONLY constructor
}
impl IrqLine {                          // enable/ack need the token
    pub fn enable(&self);
}
```

This is the microkernel idea of capabilities at function granularity
(see §10.4). Apply incrementally: IRQ lines first (they are being
rewritten for x86 anyway), then timer channels, then IOMMU/dma handles.

### 1.5 MMIO access without aliasing

MMIO regions are never `&mut T` (Rust aliasing rules fight hardware
semantics). Access goes through address-based volatile helpers owned by
the driver type:

```rust
impl VirtioMmio {
    fn read32(&self, off: usize) -> u32 {
        unsafe { core::ptr::read_volatile((self.base + off) as *const u32) }
    }
}
```

One unsafe primitive per device, everything above it safe.

---

## 2. Error handling discipline

### 2.1 `Result` inside the kernel, errno at the edge

- **Internal kernel APIs return `Result<T, KernelError>`** (a newtype
  over the errno value set, with kind-tests: `is_recoverable()`,
  `should_kill_task()`).
- **errno conversion happens exactly twice**: at the syscall return
  path, and at ABI boundaries that genuinely speak errno (e.g.
  `copy_to_user` returning short counts).
- Rationale: `Result` forces the caller to consider the failure branch;
   `-errno as i64` return values make ignoring failure the path of
   least resistance (we have real bugs of this class in git history).

New code (x86_64 arch files, binder, all new subsystems) follows this
from day one; existing subsystems migrate per §9-F4. Do **not** churn
all 24k lines of syscall handlers at once.

### 2.2 Panic policy by context

| Context | Policy |
|---|---|
| Boot (before scheduler) | panic allowed — fail fast, print, halt |
| Task context, invariant "impossible" | panic allowed (it is a kernel bug; want it loud) |
| Task context, runtime failure | degrade: return error, or kill the offending task (OOM path already does this — `alloc_error_handler` design) |
| IRQ / softirq / scheduler-internal | **panic forbidden**: no allocation in the message, use the raw-line printer (`dfx::taskdump_raw_line` style), then contain |
| Per-task recoverable (bad user pointer) | never panic — EFAULT |

Rule of thumb: *a user process must never be able to panic the kernel,
and the kernel must never allocate while reporting its own death.*

### 2.3 No `unwrap`/`expect` on runtime paths

`unwrap` is an implicit invariant assertion. Policy: `unwrap`/
`expect` allowed in `#[cfg(test)]` and in code that can only run at
boot; denied elsewhere by clippy (§9-F6). Where an invariant genuinely
holds, write `debug_assert!` + a graceful path anyway.

---

## 3. `unsafe` discipline and the TCB ledger

### 3.1 Block discipline (existing rule, now formalized)

- Every `unsafe` block is **≤ 5 lines** and preceded by a safety
  contract comment stating: what is assumed, why it holds here, what
  would break it.
- Prefer `unsafe trait` + safe methods: the invariant is stated once
  on the trait, not re-argued at every call.
- Raw-pointer review checklist (from existing code-review rounds):
  lifetime of the pointee, aliasing exclusivity, alignment, provenance
  (§10.5).

### 3.2 Unsafe is a budget, so measure it

The credible number in the field: a Linux-ABI-compatible Rust kernel
reports a memory-safety TCB of ~14% of its codebase (unsafe lines +
the safe-API shims directly exposing them). We adopt the same
accounting:

- **TCB ledger script** (`scripts/tcb_ledger.py`): per subsystem,
  count `unsafe` blocks/lines and the safe wrapper surface; emit a
  table.
- Every release notes the TCB percentage in the roadmap; the number
  may only go down or be justified (new arch bring-up raises it
  temporarily — the x86_64 arch tree will; that is expected and must
  shrink as the port stabilizes).
- Unsafe added by a fix requires a comment in the PR body justifying
  why safe wasn't possible.

### 3.3 Provenance-clean uaccess

User pointers are opaque addresses, never dereferenced directly; they
flow only into the uaccess API (`access_ok` + copy helpers), whose
asm implements the fault containment. This exists on riscv64 — the
x86_64 port must match the same shape (exception-table-bracketed
copies), not "temporarily" dereference user memory because the kernel
address space happens to include it.

---

## 4. Concurrency patterns

1. **RAII guards only.** `lock_irqsave()` returns a `SpinlockGuard`
   that restores IF on drop; new `IrqGuard`/`PreemptGuard` types wrap
   the raw asm primitives. Bool-returning `save_and_disable_irq` +
   `restore_irq` pairs remain for asm-callable paths only. A guard
   cannot be forgotten; a bool can.
2. **Lock-free fast path, locked slow path.** Read-mostly state
   checks with an atomic load first, take the lock only on the slow
   path (the executor-queue pattern: empty-check before lock).
3. **TTAS spinlocks**: CAS to acquire, plain-load spin while held,
   `spin_loop()` hint, bounded-spin debug panic (companion doc A2).
4. **Wake-ordering invariant** documented at every wake site:
   extract waker → set flag (release) → wake (companion doc A5);
   SPIN models updated when the protocol changes.
5. **Per-CPU data**: one `align(64)` struct per CPU, cache-line
   partitioned, own cpu id published inside (companion doc A1).
6. **Context rules**: IRQ and softirq context performs no allocation
   and takes no sleeping locks — this is reviewable by looking at
   what a path is *allowed* to call; keep the allowed-call lists in
   the dfx docs.

---

## 5. Allocation discipline

- **Allocation is a task-context privilege.** IRQ/softirq/panic paths
  run on preallocated pools or drop work to task context (softirqd).
- Boot-path allocations are **fallible** (`try_alloc` shapes) until
  the allocator is proven; early-boot failure prints and halts.
- OOM in task context kills the task, not the machine (existing
  `alloc_error_handler` design — keep it).
- No unbounded growth without a cap in kernel-resident tables (fd
  table caps exist; audit uevent/seq structures when touched).

---

## 6. Module architecture

1. **Curated interfaces.** Each module's `mod.rs` re-exports an
   explicit list forming the module's API — not a glob of internal
   items. The arch interface layer (landed 2026-10-06) is the model:
   call sites depend on `crate::arch::*`, never on a backend.
2. **Dispatch style**: `cfg` for compile-time alternatives (arch
   backends); traits only where implementations genuinely coexist at
   runtime (`IrqChip`, scheduler classes) or for test doubles. Trait
   objects in hot paths require a comment saying why.
3. **One-way layering**: `syscall → subsystems (fs/mm/net/...) → arch
   interface → backends`. Upward references are a review blocker.
   (Cargo workspace split into crates is the eventual mechanical
   enforcement — do it after the x86 port stabilizes, not during.)
4. **File size**: split modules that pass ~1,500 lines
   (`arch/riscv64/mm/mm_ops.rs` at 1,975 and `mmu_init.rs` at 1,793
   are over; the x86_64 versions start split: pagetable / walker /
   ledger / early-init).

---

## 7. Verification is part of the API

- **Pure codec functions** (PTE encode/decode, checksums, bitmap ops)
  are written as free pure functions *beside* the stateful code, so
  Kani/proptest harnesses attach without heroics. The x86_64 PTE codec
  is designed this way from the first commit; the riscv64 codec gets
  extracted during its next touch.
- **Invariant IDs** (`INV-CS-*`, `INV-LOCK-*`) referenced in tests
   and in the SPIN models — an invariant without an ID is a comment;
   an ID is a contract.
- Miri-clean core structures: everything that can run under the host
   verify crate does (existing practice — extend to new lock-free
   structures).

---

## 8. Documentation and review gates

- Safety-contract comments on `unsafe` are a merge gate (reviewer
  checks the contract, not just the code).
- Optimization work carries before/after numbers and negative results
  (changelog culture — extend to perf notes).
- Decisions that shaped the tree (arch interface, march pinning, port
  gating) live in dated documents under `docs/development/`, so the
  next contributor inherits the *reason*, not just the code.

---

## 9. Funded refactor program (whole project, staged)

| # | Refactor | Scope | Gate | When |
|---|---|---|---|---|
| F1 | TCB ledger script + baseline number | `scripts/`, roadmap entry | report exists | **now** (with x86 port) |
| F2 | Newtype sweep: raw addresses in generic code → `PhysAddr`/`VirtAddr`/`Pfn` | `syscall/`, `fs/`, `drivers/` | LTP parity | post-Ubuntu-gate |
| F3 | Guard migration: irqsave/preempt bools → RAII guards | hot paths first (`fs/bio.rs`, `drivers/virtio/`) | LTP + soak | post-Ubuntu-gate |
| F4 | `Result<T, KernelError>` for internal APIs | new code now; one subsystem at a time after | per-subsystem | ongoing |
| F5 | Split oversized modules; curated `mod.rs` exports | `arch/*/mm/*`, `fs/vfs.rs` | builds + tests | with x86 mm work |
| F6 | Clippy lint set: deny `unwrap`/`expect` outside boot/tests | workspace | clean build | after F4 starts |
| F7 | Dual-arch CI matrix (riscv64 + x86_64 both green) | CI | both targets | with x86 first boot |

Explicitly rejected: a single big-bang "rewrite everything as safe
idiomatic Rust" branch. Refactors that cannot land in gated increments
do not land.

---

## 10. Safety innovations beyond the classic microkernel

The question this section answers: *are there architectures that get
microkernel-grade safety without paying microkernel IPC cost — and
what of them applies to Rux?*

### 10.1 Safe-language isolation — the framekernel line

The most direct answer for a Rust kernel: **let the compiler be the
isolation boundary instead of address spaces**. A Linux-ABI-compatible
Rust kernel (Asterinas, USENIX ATC'25) demonstrates this at scale: one
address space like a monolith, but split into a small privileged
*framework* and unprivileged *services*, with the boundary enforced by
safe Rust (their OSTD development framework: safe thread/CPU context
management, safe user-memory access, deferred work). Reported result:
210+ Linux syscalls, Linux-comparable performance, memory-safety TCB
~14% of the codebase.

**Rux takes**: not the architecture (we are and stay a monolithic
Linux-compatible kernel), but the two mechanisms that produced their
number — (a) a measured TCB ledger (§3.2), (b) the guard/context API
discipline of OSTD-style frameworks (§2, §4). Every point of unsafe
we retire moves us along the same axis without any redesign.

### 10.2 Verified kernels — the seL4 / Verus line

- **seL4** (SOSP'09, C + Isabelle) remains the reference: full
  functional-correctness proof of a general-purpose microkernel, plus
  capability-based access control with formal isolation proofs. The
  transferable ideas are not the proofs (not portable to our scale)
  but the *method*: enumerate the TCB, then shrink what you reason
  about.
- The Rust-native line: **Verus**-verified kernels, **"Verified
  Paging for x86-64 in Rust"** (ETH, 2022 — exactly our current
  component), **Atmosphere** (2023) toward practical verified Rust
  kernels, page-table verification for a Rust TEE (Dai et al., 2024),
  and model checking of kernel concurrency (**Converos**, ATC'25).

**Rux takes**: our existing Kani/proptest/SPIN/Miri stack is the same
bet at smaller scale. The rule that makes it pay later: verification-
shaped APIs (§7) — pure codecs, typed walkers, invariant IDs.

### 10.3 Intra-kernel hardware isolation — compartments without IPC

A research line directly relevant to "microkernel-like safety inside a
monolith", all roughly 2020–2023:

- **MPK/PKS (Intel)**: per-thread permission keys over 4KB-granule
  page protections — domain switch is a register write, ~ns scale.
- **Donky** (USENIX Security'20): "domain keys", hardware-software
  codesign for in-process isolation on RISC-V and x86.
- **FlexOS** (ASPLOS'22): isolation as a *configuration-time* dial —
  Lightweight Isolation Domains backed by MPK or page tables, built on
  the Unikraft libOS; same OS image re-compartmentalized per workload.
- **xMP** (USENIX ATC'22): intra-kernel sandboxing with
  compiler/verifier-assisted memory isolation.
- **HAKC** (CCS'22): ARM PAC/MTE-based kernel compartmentalization.

**Rux position**: interesting post-Phase-3 experiment, not current
work. The concrete candidate: once the OpenHarmony port reaches
Phase 2 (binder + foreign-derived userspace), evaluate a
**PKS-gated domain** for the binder driver and any parsing-of-untrusted-
input path (binder transactions are attacker-controlled by
definition). First step when we get there: check QEMU q35 CPU model
PKS support (CPUID leaf 7; TCG support is incomplete — may need
`-cpu max`). The architecture hook to keep open: keep binder's I/O
buffers behind one driver type so a domain boundary could wrap it.

### 10.4 Capability models

- **seL4**: capabilities as first-class, unforgeable references with
  derivation trees; authority = what caps you hold.
- **Redox** (Rust microkernel): resources as URL-schemes; the fd table
  is the capability space.
- Linux/Rux already speak a capability dialect: file descriptors,
  signal-target rules, namespace handles. The practical discipline is
  §1.4 — function-granularity capability tokens for kernel-internal
  authority (IRQ lines, timers, DMA handles) — plus an **authority
  audit**: enumerate every "god handle" (raw `&Task`, global
  registries) in the tree and give each a story. Do the audit when the
  x86 port lands, not before.

### 10.5 Provenance and the unsafe contract

Rust's formal memory model work (Stacked Borrows / provenance line,
Miri as the executable checker) is the safety story for the unsafe
code we *must* keep (uaccess asm, page-table walking, MMIO). The rule:
**Miri-clean wherever runnable** (verify crate), and provenance-
disciplined elsewhere: pointers derived once at the boundary
(`phys_to_virt`), never re-derived ad hoc mid-function.

### 10.6 Adoption summary

| Innovation | Rux decision | When |
|---|---|---|
| Safe-language TCB shrink + ledger | **adopt** (§3.2) | now |
| OSTD-style guard/context discipline | **adopt** (§2, §4) | now (new code), staged (old) |
| Verification-shaped APIs + Kani/SPIN/Miri | **adopt** (§7) | ongoing |
| Function-granularity capabilities | **adopt incrementally** (§1.4) | with x86 IRQ rewrite |
| PKS/MPK compartments for untrusted parsers | experiment | post Phase-3 (binder), QEMU check first |
| Classic microkernel decomposition | **reject** | — (Linux-compat monolith is the mission) |
| Unikernel/libOS decomposition | **reject** | — |

---

## Appendix: primary sources

- Asterinas — [framekernel architecture](https://asterinas.github.io/book/kernel/the-framekernel-architecture.html),
  [ATC'25 paper](https://www.usenix.org/system/files/atc25-peng-yuke.pdf),
  [arXiv:2506.03876](https://arxiv.org/abs/2506.03876), [LWN](https://lwn.net/Articles/1022920/)
- Theseus — [theseus-os.com](https://www.theseus-os.com) (intralingual design, state-spill management)
- Donky — [USENIX Security'20 paper](https://www.usenix.org/system/files/sec20-schrammel.pdf)
- FlexOS — ASPLOS'22, project page project-flexos.github.io (LVDs, MPK-backed compartments on Unikraft)
- Verified paging for x86-64 in Rust — ETH Research Collection, 2022
- seL4 — SOSP'09 (capability microkernel, full functional correctness proof)
- Rust for Linux — kernel documentation and RFC history (in-tree since 6.1)
