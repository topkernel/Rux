# Rux

<div align="center">

**A Linux-ABI-compatible operating-system kernel, written from scratch in Rust.**

Subsystems are implemented by AI agents under human architecture review and merge gates.

[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-riscv64%20%7C%20x86__64%20%7C%20aarch64-informational.svg)](#quickstart)
[![Tests](https://img.shields.io/badge/tests-LTP%201%2C139%20%2B%20suite%201%2C584-brightgreen.svg)](#testing)
[![Code](https://img.shields.io/badge/code-~421%2C000%20lines-blue.svg)](docs/architecture/structure.md)

Reports & documentation: **<https://topkernel.github.io/Rux/>** (中文 / EN)

</div>

---

## Design rationale

Rux is built around three properties that must hold simultaneously, not as trade-offs but as joint constraints:

- **Monolithic performance floor.** A single address space, a single scheduler, and zero IPC overhead on the hot path. No message-passing tax is paid for isolation that the language already provides.
- **The Linux ABI as a binary contract.** Unmodified userland runs unmodified — not recompiled, not patched, not wrapped. The contract is enforced at the syscall boundary and verified by booting real distribution images.
- **Rust's memory safety as an auditable property.** The portion of the kernel that requires human review is maintained as a ledger with named invariants, registered divergences, and CI-enforced gates — not as an informal claim of coverage.

Most Rust kernels optimize for one of these. A teaching kernel demonstrates mechanisms; a microkernel prototype explores isolation; a compatibility layer reports syscall counts. Rux treats all three as simultaneous acceptance criteria.

## ABI compatibility: the contract is boot, not syscall count

The governing principle is stated in one line: the ABI is the contract; the internals are free.

Outward, every userland-observable interface is contractual: syscall numbers and semantics, ELF and auxv, futex, POSIX and SysV IPC, binder, on-disk ext4, the procfs/sysfs/debugfs/tracefs layouts — down to the `uname` strings that glibc's dynamic loader parses during startup. **349 system calls** are implemented on the asm-generic table, with x86_64 native numbering translated onto it. A full LTP scan of the 1,869-case compatibility list passes **1,139**.

Inward, no constraint is inherited from the mainstream's internal history. Every internal decision that diverges from the common design is registered with its compatibility evidence and its guard: **29 registered design divergences**, each gated by the LTP families it touches and by unmodified-software boots.

The verification method carries more weight than the count. A curated syscall subset proves nothing about binary compatibility; unmodified distribution images prove everything:

| Target | Result |
|---|---|
| **Ubuntu 22.04** | Interactive GNOME Shell desktop (riscv64; full chain passes a 2100 s, smp4, zero-panic gate) |
| **Ubuntu 24.04** | Multi-user and graphical targets under systemd 255 (x86_64, `make ubuntu-run`) |
| **OpenHarmony** | Full 135+ service mesh up to an interactive launcher — bootanimation, hiview, binder mesh, input pipeline — all built by their original toolchains (`make oh-run`) |

<p align="center">
  <img src="docs/images/ubuntu-gnome-shell-frame-2026-10-27.png" width="42%">&nbsp;
  <img src="docs/images/oh-launcher-2026-10-31.png" width="42%">
</p>
<p align="center"><sub>Unmodified Ubuntu 22.04 (GNOME Shell) and OpenHarmony (launcher) — one kernel underneath both.</sub></p>

## AI-written code as an engineering method

Every subsystem — scheduler, memory management, ext4, binder, network stack, verification harness — plus the documentation and this site were written by AI agents. The human role is goal-setting, architecture review, and merge authority.

The working model:

- **parallel agents**, each in its own git worktree on its own branch; approximately **200 worktrees** are live;
- **one merge law**, written in [AGENTS.md](AGENTS.md) and applied identically to machine and human: three-architecture build, full in-kernel suite, tiered performance gates, data-integrity batteries — all must pass before anything lands;
- **every performance claim carries same-machine A/B numbers**, with the rig named;
- **every divergence is registered**; negative results remain in the record with their receipts.

This is not "an assistant filling in a few lines." It places AI where subsystems are implemented and keeps the human at architecture review and the merge gate. Whether that model holds at 400,000 lines remains under observation — so far it has produced a kernel that boots a real desktop.

## Safety: the itemized bill, not the slogan

**The surface.** The kernel tree (`kernel/`) is 421,180 lines ([structure](docs/architecture/structure.md)); the ledger below counts `kernel/src` Rust at 362,762 lines on its own basis (non-blank, generated files excluded) — the shares use that basis:

| Series | Lines | Share |
|---|---:|---:|
| `unsafe { … }` block bodies | 13,623 | 3.8% |
| `unsafe fn` / `unsafe impl` bodies | 3,517 | 1.0% |
| **Dual series — the honest total** | **16,712** | **4.6%** |

The unsafe that remains is a territory, not a population: **171 modules are compiler-enforced zero-unsafe** (`#![forbid(unsafe_code)]`; CI ratchets the set — a file that regresses fails the build), and what is left concentrates in the Framework layer — per-arch entry, page-table, and uaccess edges plus one named edge module per subsystem, each block under a numbered INV contract. 67% of unsafe blocks carry a written `// SAFETY:` contract, and CI fails when a region appears in the tree without a row in the [verification ledger](docs/development/unsafe-verification.md). The defect classes that dominate C-kernel CVE history — indexing overrun, iterator invalidation, format-string injection, wild pointer arithmetic — are **zero in safe code** (the adjudicated record behind this claim: [defect taxonomy](https://topkernel.github.io/Rux/en/security/taxonomy.html)).

**The proofs.** 164 Kani harnesses discharge core unsafe routines for all inputs; 1,184 host-side tests run in CI; loom enumerates interleavings for the mutex, PI-chain, and timer primitives; 4 SPIN models check the futex, interrupt/preempt, lock-ordering, and scheduler models; Miri runs nightly.

The claim is not "90% safe." The claim is: here is exactly what the remaining unsafe surface is, where it concentrates, which tool watches it, and what it has cost so far. Details: [safety & performance ledger](docs/architecture/safety-and-performance.md) · [security taxonomy](https://topkernel.github.io/Rux/en/security/taxonomy.html).

## Performance: same machine, same accelerator

Every row is measured against Linux on the same host, the same image, and the same static binaries — alternating boots, medians. The boards carry the method and a per-row attribution of each remaining gap.

| Suite | Result |
|---|---|
| lmbench (27 rows) | 13 ahead — semaphore 8.97×, context switch 5.1–6.3×, pipe 5.84×, page fault 2.8×, page-cache file read 1.45–1.54×; 13 behind or at parity, worst file creation 22–26× and loopback socket latency 2.1–2.2×; one row (loopback connect) is a repaired defect |
| fio 3.36 | 4 KiB write+fsync 1.24× ahead; 1 MiB buffered write 1.19–1.25× ahead (11.7 GiB/s); direct 1 MiB sequential write 0.75–0.83×, direct read 0.83× |
| Geekbench 5 | single-core 114%, multi-core 107% (4-CPU pinned) |
| rt-tests v2.11 | cyclictest SCHED_OTHER average 1.5× ahead; SCHED_FIFO minimum wins every clean pass, maximum 10× ahead, average 1.15–1.3× behind; hackbench 1.06–1.16× ahead on all three forms; pi_stress runs 273k–383k priority inversions cleanly |
| Internal microbenchmarks | context switch 4.97 → 2.07 µs, page fault 0.468 → 0.029 µs, GRQ critical section ≈60 ns, buffer-cache hit rate 99.1% |

Full boards: [fio](https://topkernel.github.io/Rux/performance/fio.html) · [rt-tests](https://topkernel.github.io/Rux/performance/rttests.html) · [lmbench](https://topkernel.github.io/Rux/performance/lmbench.html) · [Geekbench](https://topkernel.github.io/Rux/performance/geekbench.html) · [microbenchmarks](https://topkernel.github.io/Rux/performance/microbench.html)

## Testing

| Class | Scale |
|---|---|
| LTP compatibility list | **1,139 of 1,869** |
| In-kernel suite | **1,584 cases**, every merge |
| Architectures | riscv64 · x86_64 · aarch64 — build + boot regression, every push |
| Formal verification | 164 Kani harnesses · 1,184 host tests · 4 SPIN models · 8 loom scenarios · Miri nightly |
| Data integrity | fio `--verify`, e2fsck, hard-kill persistence batteries |
| Stress fleet | `make ci` — 32 classes including fsfuzz, hours unattended |

Reports: [test index](https://topkernel.github.io/Rux/testing/index.html).

## What is in the 419,000 lines

The `kernel/` tree holds 679 files and 419,958 lines, of which 408,864 are Rust.

- **Scheduler** — a single global run queue (D-1), sized for 8–16 CPU personal devices: one priority-ordered pick, no balancer, no migration. Measured critical section ≈60 ns; the contention ceiling extrapolates to roughly 950 cores at the observed 241–273 ns hold.
- **Memory management** — buddy page allocator with a magazine small-object allocator, a global page cache, per-task VMAs, reclaim.
- **Filesystem** — ext4 read/write, jbd2, an async batching block layer, a per-inode page cache with a lock-free dcache.
- **IPC and binder** — a complete binder (it carries OpenHarmony's entire service mesh), unix sockets, pipes, SysV IPC.
- **Network** — a self-written TCP/IP stack with socket-API semantics matched to the ABI.
- **Drivers** — virtio-blk (PCI and MMIO, async completion rings), serial, input, and a display path that renders both the GNOME desktop and the OpenHarmony launcher.
- **Observability** — a unified trace-event framework with tracefs aligned to the reference semantics, plus a symbol table, debugfs, and a task dump.
- **Porting layer** — x86_64, riscv64, and aarch64; approximately 90% of the kernel is architecture-independent.

Because the host can use KVM, x86_64 is the primary platform for both function and performance work; riscv64 and aarch64 carry the build and regression gates.

## What is not finished

- **LTP**: 1,139 of 1,869 pass. Several hundred still fail; the TIMEOUT class is adjudicated case by case, and each failure that is not ours is recorded as such.
- **lmbench**: 13 rows trail, the largest deficit file creation at 22–26× and loopback socket latency at 2.1–2.2×. Each row carries a measured decomposition of the gap.
- **Direct I/O**: 1 MiB sequential streams sit at 0.75–0.83× of the reference.
- **The unsafe surface**: 4.6% of the kernel (16,712 lines) still depends on human review. The campaign target is ≤5,000 lines (≤1%) with a per-block irreducibility proof — which primitive owns the block, why no safe shape exists, the reference counterpart — recorded in the [ledger](docs/development/unsafe-verification.md); the ladder is in [the plan](docs/development/unsafe-5000-plan.md).
- **Formal verification**: Kani covers the core unsafe primitives, not every key path; loom and SPIN cover the concurrency primitives.
- **The AI working model** at 400,000 lines is under observation, not settled.
- **The 29 registered divergences** need their compatibility evidence kept current as the kernel moves.

## Quickstart

```bash
# Toolchain: Rust nightly, QEMU; musl cross-tools bundled under toolchain/
make build                   # kernel (default riscv64)
make user rootfs             # userspace + minimal rootfs image
make run                     # boot to a shell
PLATFORM=x86_64 make build   # x86_64 / aarch64
make test                    # 1,584-case in-kernel suite
make ubuntu-run              # boot the Ubuntu desktop
make oh-run                  # boot OpenHarmony to the launcher
make ci                      # full stress pipeline, unattended
```

The serial console is the primary log (`console=ttyS0`); on panic the trap printer emits epc/ra/sp/badaddr and stack words for `riscv64-linux-gnu-addr2line`.
[Getting started](https://topkernel.github.io/Rux/guides/getting-started.html) walks a first session end to end.

## Contributing

Pull requests are welcome, human or machine; the bar does not distinguish.
Build on three architectures, keep the suite green, keep performance rows in band or attach the numbers that improved, keep the integrity batteries clean. If a patch makes the kernel faster — show the rig.
