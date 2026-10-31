# Rux

<div align="center">

**A Linux-compatible kernel, ~310,000 lines of Rust — written entirely by AI.**

[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-riscv64%20%7C%20x86__64%20%7C%20aarch64-informational.svg)](#platforms)
[![Tests](https://img.shields.io/badge/tests-4%2C200%2B%20cases-brightgreen.svg)](#numbers)
[![Code](https://img.shields.io/badge/code-~310%2C000%20lines-blue.svg)](docs/architecture/structure.md)

Unmodified Ubuntu 22.04 boots to a GNOME Shell desktop.
Unmodified OpenHarmony boots its full 135+ services.
No userspace patches, rebuilds, or configuration changes.

</div>

---

## Overview

- **Developed entirely by AI agents.** All code — scheduler, memory
  management, ext4, binder, the network stack, the verification
  harnesses — was written by AI agents under human direction. Humans
  set objectives, review changes, and hold merge authority; agents
  implement, test, and debug.
- **Linux ABI compatibility.** 345+ system calls with semantics
  matched against the reference implementation; ELF/auxv, futex,
  SysV and POSIX IPC, binder, on-disk ext4, and the `/proc`,
  `/sysfs`, `/debugfs`, tracefs layouts, down to the `uname` strings
  glibc's loader keys on. LTP: **1,005 / 1,869**.
- **The ABI is the contract; the internals are a clean-room design.**
  Where an internal design differs from the mainstream, the
  divergence and its compatibility evidence are recorded in the
  [design-divergence register](docs/architecture/design-divergences.md)
  (13 entries). Scheduling, for example, uses a single global run
  queue rather than per-CPU queues with load balancing. On the 2–4
  vCPU systems Rux targets, this eliminates work stealing and
  migration entirely; the measured queue hold is 5.1–6.1 µs, flat
  across load. Per-CPU queues with balancing address larger core
  counts — a difference of design intent and target scenario, not of
  quality.
- **Safety discipline with machine-checked evidence.** ~4,900
  `unsafe` sites, each with a written safety contract. Kani proves
  core unsafe code for all inputs (157 harnesses); proptest (1,088+),
  SPIN (4 models), and Miri support it. Mirror-sync tooling keeps the
  proofs pinned to the code they cover.
- **One source tree, three architectures.** riscv64 (primary),
  x86_64, aarch64 — each to a booting shell; Ubuntu runs on two of
  the three.

---

## Ubuntu 22.04 → GNOME Shell desktop

![GNOME Shell desktop on Rux](docs/images/ubuntu-gnome-shell-frame-2026-10-27.png)

*Unmodified Ubuntu userland: glibc, Xorg, D-Bus, GNOME Shell, mutter,
interactive xterm. Sessions hold 45 minutes with zero panics.*

```bash
make ubuntu-image && make ubuntu-run    # boot-to-login ~4.5 s, login: root / rux
```

## OpenHarmony → launcher

![OH launcher home screen](docs/images/oh-launcher-2026-10-31.png)

*Unmodified OH x86_64_virt image: samgr, foundation, render_service,
hilogd, appspawn, hdf devhosts — 135+ services, launcher home screen,
`hdc shell` to the live guest.*

---

## Getting started

Prerequisites: stable Rust (with the `riscv64gc-unknown-none-elf` and
`riscv64gc-unknown-linux-musl` targets), QEMU ≥ 5.0, and
`riscv64-linux-gnu-gcc`.

```bash
git clone https://github.com/topkernel/rux.git && cd rux
make build            # riscv64 kernel
make test             # in-kernel unit tests under QEMU
make run              # boot the minimal rootfs to an interactive shell
```

C probe sources and the x86_64 musl sysroot ship as tarballs and are
extracted automatically on the first `make user` / `make test`. Full
details: [docs/guides/getting-started.md](docs/guides/getting-started.md).

## Driving this repo with an AI agent

This repository is maintained **by AI agents**. Everything an agent
needs — environment, build map, verification ladder, coding contracts,
debugging toolkit — is in one file: **[AI_GUIDE.md](AI_GUIDE.md)**.
Point your agent at it first; then use any of these prompts as-is:

> Read AI_GUIDE.md, then build the kernel for riscv64, run
> `make test`, and boot `make run` to a shell. Report the test pass
> count and any failure with serial-log excerpts.

> Read AI_GUIDE.md. Here is the failure I observed: `<paste panic
> excerpt / test name / LTP case>`. Symbolize it, find the root
> cause, fix it, and verify with `make test` plus a boot. Commit in
> English per AGENTS.md.

> Read AI_GUIDE.md. Implement `<syscall or feature>` to the POSIX
> contract, add a guest-side probe under `test/`, extend the
> in-kernel tests, and pass the full verification ladder.

> Read AI_GUIDE.md and AGENTS.md. Review `kernel/src/<subsystem>`
> against the syscall contract and the merge-gate checklist; report
> findings as file:line references with severity.

---

## Platforms

| | riscv64 | x86_64 | aarch64 |
|---|---|---|---|
| Boot to shell | ✅ | ✅ | ✅ |
| Ubuntu / GNOME | ✅ | ✅ | bring-up |
| OpenHarmony | — | ✅ | — |
| In-kernel tests | 1,122 | ✅ | 1,119 |

```bash
make build                        # riscv64
make build-x86                    # x86_64
make build PLATFORM=aarch64       # aarch64
make test [PLATFORM=…]            # in-kernel unit tests
make verify && make kani          # proofs
```

---

## Numbers

| | |
|---|---|
| Kernel source | ~310,000 lines, 422 files |
| Syscalls dispatched | 345+ |
| Kernel unit tests | 1,122 (riscv64) · 1,119 (aarch64), all green |
| Formal verification | 1,116+ functions · 157 Kani proofs · 4 SPIN models |
| Linux LTP | **1,005 / 1,869** (35 → 524 → 801 → 945 → 1,005) |
| Swap under load | 2,064 MiB working set through 256 MB swap, verified |
| Boot to Ubuntu login | ~4.5 s (4-CPU MTTCG) |
| Design divergences | 13 registered, each with compatibility evidence |

---

## Documentation

[Getting Started](docs/guides/getting-started.md) ·
[Roadmap](docs/progress/roadmap.md) ·
[Design Divergences](docs/architecture/design-divergences.md) ·
[Formal Verification](docs/development/formal-verification.md) ·
[Lock Ordering](docs/architecture/lock-ordering.md) ·
[Review Coverage](docs/development/review-coverage.md) ·
[Changelog](docs/progress/changelog.md)

---

## License

MIT — see [LICENSE](LICENSE).

<div align="center">

**Human sets the goal. AI writes the kernel.**

[Project Home](https://github.com/topkernel/rux) · [Issues](https://github.com/topkernel/rux/issues)

</div>
