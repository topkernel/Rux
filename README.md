# Rux

<div align="center">

**A Linux-compatible kernel, ~360,000 lines of Rust — written entirely by AI.**

[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-riscv64%20%7C%20x86__64%20%7C%20aarch64-informational.svg)](#platforms)
[![Tests](https://img.shields.io/badge/tests-4%2C200%2B%20cases-brightgreen.svg)](#numbers)
[![Code](https://img.shields.io/badge/code-~360%2C000%20lines-blue.svg)](docs/architecture/structure.md)

Unmodified Ubuntu 22.04 boots to a GNOME Shell desktop.
Unmodified OpenHarmony boots 135+ services to an interactive launcher.
No userspace patches, rebuilds, or configuration changes.

**Documentation & reports: <https://topkernel.github.io/Rux/>** (中文 / EN)

</div>

---

## Overview

- **Developed entirely by AI agents.** All code — scheduler, memory
  management, ext4, binder, the network stack, the verification
  harnesses — was written by AI agents under human direction. Humans
  set objectives, review changes, and hold merge authority; agents
  implement, test, and debug. Over 1,000 commits on main since
  November 2026 alone, from 100+ numbered agent sessions.
- **Linux ABI compatibility.** 345+ system calls with semantics
  matched against the reference implementation; ELF/auxv, futex,
  SysV and POSIX IPC, binder, on-disk ext4, and the `/proc`,
  `/sysfs`, `/debugfs`, tracefs layouts, down to the `uname` strings
  glibc's loader keys on. LTP: **1,059 / 1,869**.
- **The ABI is the contract; the internals are a clean-room design.**
  Where an internal design differs from the mainstream, the
  divergence and its compatibility evidence are recorded in the
  [design-divergence register](docs/architecture/design-divergences.md).
  Scheduling, for example, uses a single global run queue rather
  than per-CPU queues with load balancing. On the 2–8 vCPU systems
  Rux targets, this eliminates work stealing and migration entirely;
  the measured queue hold is 140 ns after the opt-level 2 flip,
  flat across load. Per-CPU queues with balancing address larger
  core counts — a difference of design intent and target scenario,
  not of quality.
- **Safety discipline with machine-checked evidence.** ~5,300
  `unsafe` sites, each with a written safety contract. Kani proves
  core unsafe code for all inputs (157 harnesses); proptest (1,088+),
  SPIN (4 models), and Miri support it. Mirror-sync tooling keeps the
  proofs pinned to the code they cover. The bug taxonomy tracks that
  every memory-corruption-class bug in the project's history lived
  in audited unsafe — none in safe code.
- **One source tree, three architectures.** riscv64 (primary),
  x86_64, aarch64 — each to a booting shell; Ubuntu and
  OpenHarmony both run on x86_64; Ubuntu also runs on riscv64.

### Architectural highlights

The ABI is the contract; the internals are original Rust-native
designs. The essence, each with measured evidence in the
[Rust-native register](docs/architecture/rust-native-register.md):

- **Types eliminate bug classes.** Timer-class misuse (one-shot vs
  periodic, IRQ vs task context) and use-after-cancel are compile
  errors; the mutex handoff state machine is loom/Kani model-checked
  — the first run found a real bug in 90 s.
- **Ownership is the fast path.** Allocation-free dentry and timer
  walks took mmap 72→30 µs under load and cut timer-lock holds
  25–33% per operation.
- **Safe by construction.** Userspace-writable strings cannot panic
  the kernel printer; diagnostics are bounded and allocation-free,
  so the observer cannot become the pathology.
- **Lock-free reads on shared structures.** The dcache walk runs on
  typed reader banks (WalkClaim) instead of RCU-style epochs:
  stat 0.64→0.42 µs, open 1.13→0.89 µs — reader progress is a type
  property, not a protocol discipline.
- **Lazy TLB shootdown.** Each user-mm operation (munmap, mprotect,
  fork COW-downgrade) broadcasts to only the CPUs currently running
  that mm, not all CPUs: 3.9M → 18K IPIs on an 8-CPU OpenHarmony
  boot (-99.5%), with zero corruption signatures across 10
  verification boots.
- **Live vDSO.** 21.3 million `clock_gettime` calls per boot run
  at 66–74 ns in userspace instead of a 3 µs syscall — the vDSO
  page is built at boot and mapped into every address space.

---

## Ubuntu 22.04 → gdm login → GNOME Shell desktop

![gdm login prompt on Rux](docs/images/gdm-gnome-login-frame-2026-10-27.png)
![GNOME Shell desktop on Rux](docs/images/ubuntu-gnome-shell-frame-2026-10-27.png)

*Unmodified Ubuntu userland: glibc, Xorg, D-Bus, GNOME Shell, mutter,
interactive xterm. The gdm greeter runs the stock chain end to end —
udevd → logind (CreateSession with a real session fd) → gdm3 →
PAM → gdm-x-session → gnome-session; the login prompt renders and
holds (login: rux). Desktop sessions hold 45 minutes with zero panics.*

```bash
make ubuntu-image && make ubuntu-run    # boot-to-login ~4.5 s, login: root / rux
```

## OpenHarmony → launcher

![OH launcher home screen](docs/images/oh-launcher-2026-11-10.png)

*Unmodified OH x86_64_virt image: samgr, foundation, render_service,
hilogd, appspawn, hdf devhosts — 135+ services, launcher home screen,
`hdc shell` to the live guest. Best measured `launcher.ready` on a
pristine first boot: **16.3 s** (from 239 s at campaign start — a 14.6x
improvement through kernel-side fixes alone: TLB lazy shootdown, vDSO,
allocator fast paths, opt-level 2, signal/binder print gating).*

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
| In-kernel tests | 1,319 | ✅ | 1,119 |

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
| Kernel source | ~360,000 lines (446 files) + 26,500 verify |
| Syscalls dispatched | 345+ |
| Kernel unit tests | 1,319 (riscv64), all green |
| Formal verification | 157 Kani proofs · 4 SPIN models · 1,088+ proptests |
| Linux LTP | **1,059 / 1,869** (35 → 524 → 801 → 945 → 1,005 → 1,059) |
| `unsafe` sites | ~5,300, each with a written safety contract |
| Swap under load | 2,064 MiB working set through 256 MB swap, verified |
| Boot to Ubuntu login | ~4.5 s (4-CPU MTTCG) |
| OH launcher.ready | **16.3 s** best (from 239 s; 14.6x through kernel fixes) |
| OH bootanimation.ready | **5.7 s** |
| TLB shootdown IPIs | 3.9M → 18K per OH boot (lazy mm_cpumask) |
| Design divergences | 18 registered, each with compatibility evidence |

---

## Documentation

[Getting Started](docs/guides/getting-started.md) ·
[Roadmap](docs/progress/roadmap.md) ·
[Design Divergences](docs/architecture/design-divergences.md) ·
[Rust-Native Register](docs/architecture/rust-native-register.md) ·
[Safety & Performance](docs/architecture/safety-and-performance.md) ·
[Test Reports](docs/test/README.md) ·
[Lock Ordering](docs/architecture/lock-ordering.md) ·
[Changelog](docs/progress/changelog.md)

---

## License

MIT — see [LICENSE](LICENSE).

<div align="center">

**Human sets the goal. AI writes the kernel.**

[Project Home](https://github.com/topkernel/rux) · [Issues](https://github.com/topkernel/rux/issues)

</div>
