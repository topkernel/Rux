# Rux

<div align="center">

**A Linux-compatible kernel, ~391,000 lines of Rust — written entirely by AI.**

[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-riscv64%20%7C%20x86__64%20%7C%20aarch64-informational.svg)](#platforms)
[![Tests](https://img.shields.io/badge/tests-4%2C200%2B%20cases-brightgreen.svg)](#numbers)
[![Code](https://img.shields.io/badge/code-~391%2C000%20lines-blue.svg)](docs/architecture/structure.md)

Unmodified Ubuntu 22.04 boots to a GNOME Shell desktop.
Unmodified OpenHarmony boots 135+ services to an interactive launcher.
No userspace patches, rebuilds, or configuration changes.

**Documentation & reports: <https://topkernel.github.io/Rux/>** (中文 / EN)

</div>

---

## Design principles

1. **The ABI is the contract.** System-call numbers and semantics, ELF/auxv,
   futex, SysV/POSIX IPC, binder, on-disk ext4, and the procfs/sysfs/
   debugfs/tracefs layouts match the reference — down to the `uname`
   strings glibc's loader keys on. Unmodified distributions are the
   acceptance test.
2. **The internals are a clean-room Rust design.** Where an internal
   design differs from the mainstream, it is registered with its
   compatibility evidence in the
   [design-divergence register](docs/architecture/design-divergences.md)
   (19 entries) — e.g. a single global run queue instead of per-CPU
   queues with balancing (no work stealing or migration at the 2–8
   vCPU scale Rux targets; 140 ns measured hold, flat across load).
3. **Safety claims carry machine-checked evidence.** The unsafe surface
   (3,158 service + 1,340 framework tokens) is enumerated and under
   written `// SAFETY:` contract; Kani proves core unsafe code for all
   inputs, and the
   [bug taxonomy](docs/architecture/safety-and-performance.md)
   adjudicates every historical commit: zero pointer-expression
   defects (out-of-bounds, wild pointers, format strings) in safe code.
4. **Performance is measured, or it does not merge.** Every change
   passes a three-tier gate (syscost census / pbench / IF=0) over six
   guarded domains; baselines move only through merges that passed
   their own tier.
5. **Agents write the code; humans set goals, review, and merge.**
   Every line — scheduler, mm, ext4, binder, network stack,
   verification harnesses — was written by AI agents in per-task
   worktrees against the merge gate above.

## What runs today

**Ubuntu 22.04 → gdm → GNOME Shell (x86_64, KVM; riscv64 TCG)**

![GNOME Shell desktop on Rux](docs/images/ubuntu-gnome-shell-frame-2026-10-27.png)

Stock glibc/Xorg/D-Bus/gnome-session chain end to end, interactive
keyboard and xterm; boot-to-login ~4.5 s; sessions hold 45+ minutes
with zero panics.

**OpenHarmony → launcher (x86_64)**

![OH launcher home screen](docs/images/oh-launcher-2026-10-31.png)

Stock OH x86_64_virt image: samgr, render_service, 135+ services,
interactive launcher, `hdc shell` into the live guest.

```bash
make ubuntu-image && make ubuntu-run    # Ubuntu GNOME desktop (login: root / rux)
make oh-image && make oh-run            # OpenHarmony launcher (images: see note)
```

> **Note (2026-11-12):** the standing OH image store lost four of its
> eight disks to a concurrent-session accident (vendor/updater/
> sys_prod/chip_prod). system/ramdisk/userdata were recovered or
> rebuilt; re-fetch `vendor.img` (DevEco Studio → Device Manager →
> emulator image, api12 x86_64) into `/home/william/rux-images/oh/`
> to restore `make oh-run`.

## Build and run

Prerequisites: stable Rust (`riscv64gc-unknown-none-elf`,
`riscv64gc-unknown-linux-musl` targets), QEMU ≥ 5.0,
`riscv64-linux-gnu-gcc`.

```bash
make build                 # kernel (riscv64 default)
make build-x86             # kernel (x86_64)
make run                   # minimal rootfs → interactive shell
make test                  # in-kernel test suite under QEMU
make ci                    # full unattended pass: build → boot regressions → LTP → stress
```

## Testing

| Surface | Standing |
|---|---|
| In-kernel suite (`make test`) | 1,267 riscv64 · 1,263 x86_64 · 1,119 aarch64, all green |
| Linux Test Project | **1,084 / 1,869** |
| Formal verification | 157 Kani harnesses · 1,104 proptests · 4 SPIN models · Miri |
| Performance (same-rig KVM A/B vs Linux) | lmbench: 10 of 14 paired rows at parity or ahead (best 4.2×, worst deficit 1.61×) · Geekbench 5: multi-core 7332 vs 7027 |
| fio 3.36 storage battery | 9 load classes PASS, 256 MiB full-matrix crc32 verify at zero errors |
| Stress fleet | standing 11-QEMU loop (LTP, randomized, memstorm, fsfuzz) |
| One-key CI (`make ci`) | build → suites → image → Ubuntu/OH boot regressions → full LTP → 32-class stress; a KERNPANIC anywhere halts with an alert |

Boot regressions are scripted-serial verdicts: Ubuntu (420 s, login/
gnome markers + KERNPANIC detection), OpenHarmony (dual-disk 600 s,
SA-insert count ≥ 20 liveness).

## Debugging

Serial console (`console=ttyS0`) is the primary log; panics print
CSRs, all 31 registers and a 32-frame backtrace for `addr2line`.
tracefs carries 61 events / 20 groups with reference-aligned formats;
PSI (`/proc/pressure/*`) is always on. Deadlock defense: typed lock
keys (reverse nesting does not compile), the spin watchdog
(`debug=watchdog`), SPIN models — with lockdep-lite landing on top.
`test/hunt-wedge.sh` captures hangs automatically (registers, stacks,
disassembly, preserved ELF). Full guide:
[Debugging](https://topkernel.github.io/Rux/debugging/) ·
[docs/guides/debugging.md](docs/guides/debugging.md).

## Working with an AI agent

Everything an agent needs — environment, build map, verification
ladder, coding contracts, debugging toolkit — is in
**[AI_GUIDE.md](AI_GUIDE.md)**. Point your agent at it first, e.g.:

> Read AI_GUIDE.md, then build the kernel for riscv64, run
> `make test`, and boot `make run` to a shell. Report the test pass
> count and any failure with serial-log excerpts.

> Read AI_GUIDE.md. Here is the failure I observed: `<paste panic
> excerpt / test name / LTP case>`. Symbolize it, find the root cause,
> fix it, and verify with `make test` plus a boot. Commit in English
> per AGENTS.md.

## Platforms

| | riscv64 | x86_64 | aarch64 |
|---|---|---|---|
| Boot to shell | ✅ | ✅ | ✅ |
| Ubuntu / GNOME | ✅ | ✅ | bring-up |
| OpenHarmony | — | ✅ | — |
| In-kernel tests | 1,267 | 1,263 | 1,119 |

## Numbers

| | |
|---|---|
| Kernel source | ~391,000 lines, 438 files |
| Syscalls dispatched | 345+ |
| Design divergences registered | 19, each with compatibility evidence |
| `unsafe` tokens | 3,158 service + 1,340 framework, each under a written contract |
| Trace events | 61 events / 20 groups via tracefs |
| Boot to Ubuntu login | ~4.5 s (4-CPU MTTCG) |

## Documentation

[Homepage (中文/EN)](https://topkernel.github.io/Rux/) ·
[Getting Started](docs/guides/getting-started.md) ·
[Debugging](docs/guides/debugging.md) ·
[Design Divergences](docs/architecture/design-divergences.md) ·
[Rust-Native Register](docs/architecture/rust-native-register.md) ·
[Safety & Performance](docs/architecture/safety-and-performance.md) ·
[Test Reports](docs/test/README.md) ·
[Lock Ordering](docs/architecture/lock-ordering.md) ·
[Roadmap](docs/progress/roadmap.md)

## License

MIT — see [LICENSE](LICENSE).

<div align="center">

**Human sets the goal. AI writes the kernel.**

[Project Home](https://github.com/topkernel/rux) · [Issues](https://github.com/topkernel/rux/issues) ·
[Guestbook](https://topkernel.github.io/Rux/guestbook.html)

</div>
