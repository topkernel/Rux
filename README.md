# Rux

<div align="center">

**A Linux-compatible kernel in pure Rust — and every line of it was written by AI.**

[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-riscv64%20%7C%20x86__64%20%7C%20aarch64-informational.svg)](#quickstart)
[![Tests](https://img.shields.io/badge/tests-LTP%201%2C139%20%2B%20suite%201%2C335-brightgreen.svg)](#testing)
[![Code](https://img.shields.io/badge/code-~392%2C000%20lines-blue.svg)](docs/architecture/structure.md)

Documentation & reports: **<https://topkernel.github.io/Rux/>** (中文 / EN)

</div>

---

## The contract: your binaries, untouched

Rux speaks the Linux ABI — system-call numbers and semantics, ELF and auxv,
futex, POSIX/SysV IPC, binder, on-disk ext4, the procfs/sysfs/debugfs/tracefs
layouts, down to the `uname` strings glibc's loader keys on. The acceptance
test is brutal and simple: **a stock distribution boots and runs — no ports,
no patches, no recompilation, not even a rebuild of a single package.**

Two proofs, one kernel:

- **Ubuntu** — 22.04 boots to an interactive GNOME Shell desktop on riscv64;
  24.04 boots through systemd 255 to multi-user and graphical targets on
  x86_64 (`make ubuntu-run`). Unmodified `.deb` binaries, unmodified systemd.
- **OpenHarmony** — the full 135-service mesh starts to an interactive
  launcher; bootanimation, hiview, the binder mesh and the input pipeline
  all run as built by their original toolchains (`make oh-run`).

<p align="center">
  <img src="docs/images/ubuntu-gnome-shell-frame-2026-10-27.png" width="42%">&nbsp;
  <img src="docs/images/oh-launcher-2026-10-31.png" width="42%">
</p>
<p align="center"><sub>Ubuntu 22.04 GNOME Shell (left) · OpenHarmony launcher (right), both unmodified images.</sub></p>

Inside, the design is a clean-room Rust architecture, not a translation. Where
an internal structure deliberately diverges from the mainstream, it is
registered with compatibility evidence in the
[design-divergence register](docs/architecture/design-divergences.md) — 28
entries, each gated by LTP families and unmodified-software proofs.

## Performance: measured against Linux on the same machine

Every number below is a same-machine, same-image, same-binary comparison
against Linux — alternating boots, medians. Full tables and method:
[performance reports](https://topkernel.github.io/Rux/performance/).

| Domain | Result vs Linux |
|---|---|
| ext4 durability (4 KiB write+fsync) | **1.24× faster** |
| 1 MiB buffered write | **1.19–1.25× faster** (11.7 GiB/s) |
| Directory scan (getdents, 1000 entries) | **1.95× faster** |
| Pipe / context-switch / semaphore latency | **4.6× / 4.0–4.5× / 2.9× faster** |
| Real-time wake latency (cyclictest, clean runs) | **min and max both win every clean pass** (max 10× better) |
| hackbench, all three forms | **1.06–1.16× faster** |
| Geekbench 5 | **single-core 113%, multi-core 110%** |
| 4 KiB random I/O, 1 MiB direct streams | parity band (0.83–0.97×) — open rows, each with a named, measured next term |

Sub-items lead Linux by multiples; the rows that trail carry a measured
decomposition of exactly where the remaining time goes — no hand-waving.

## Safety: the language, the architecture, the proofs

- **Language-level.** The kernel is `no_std` Rust; 12.9% of lines are
  `unsafe`, every token enumerated in the
  [TCB ledger](docs/development/tcb-ledger.md) under a written `// SAFETY:`
  contract. The bug taxonomy to date: every memory-corruption-class bug ever
  found lived inside that audited `unsafe` surface — **zero in safe code**.
- **Architecture-level.** Lock nesting is a compile-time fact (typed lock
  keys — acquiring in reverse order does not build). Critical sections are
  measured, bounded and guarded by the
  [perf gate](docs/development/perf-gate-baseline.md); lock-ordering
  invariants are documented per subsystem.
- **Machine-checked.** Kani proves core unsafe routines for all inputs;
  loom/exec models check the concurrency primitives (mutex, PI chains,
  timers) against their invariants — 1,137+ host-side proofs run in CI.

## Testing

| Class | Scale |
|---|---|
| LTP compatibility list | **1,139 passing / 1,869** |
| In-kernel test suite | **1,335 cases**, run at every merge |
| Three-architecture builds + boot regression | riscv64 · x86_64 · aarch64, every push |
| Data-integrity battery | fio `--verify`, e2fsck, hard-kill persistence runs |
| Stress fleet | `make ci` — 32 classes incl. fsfuzz, hours unattended |
| Two-sided performance battery | every row re-measured against Linux on the same platform |

## Built with AI, gated like flight software

This kernel — all ~392,000 lines — was written by AI agents under a written
contract ([AGENTS.md](AGENTS.md)): parallel worktrees, one merge law
(dual-arch build + full suite + tiered performance gates + data-integrity
verification before anything lands), every performance claim backed by a
same-machine A/B, every divergence registered, negative results kept on
record. The repository is the cockpit: bring an agent, point it at
AGENTS.md, and it works under the same law.

## Quickstart

```bash
# Toolchain: Rust nightly, QEMU, musl cross-tools (bundled under toolchain/)
make build                   # kernel (default riscv64)
make user rootfs             # userspace + minimal rootfs image
make run                     # boot to a shell
PLATFORM=x86_64 make build   # other architectures
make test                    # 1,335-case in-kernel suite
make ubuntu-run              # boot the Ubuntu desktop image
make oh-run                  # boot OpenHarmony to the launcher
make ci                      # full stress pipeline, unattended
```

Debugging: the serial console is the primary log (`console=ttyS0`); on panic
the trap printer dumps epc/ra/sp/badaddr plus stack words — symbolize with
`riscv64-linux-gnu-addr2line`. `docs/guides/getting-started.md` walks the
first session end to end.

## Contribute

Pull requests are welcome. The bar is the same for everyone and every
machine: the change builds on all three architectures, the suite stays
green, performance rows stay in band or improve with numbers attached, and
data-integrity checks stay clean. If your patch makes the kernel faster,
show the rig.

---

<div align="center">

**One ABI, sworn to the letter. One codebase, written in a language that
refuses whole bug families. One bar: the software the world already ships.**

</div>
