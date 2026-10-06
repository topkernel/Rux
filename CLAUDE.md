# Rux Kernel Project - AI Assistant Guide

This document provides project context and development guidelines for AI assistants.

## ⚠️ Core Principle (Absolutely Must Not Be Violated)

### **Full POSIX/ABI Compatibility**

This is the **highest guiding principle** for Rux kernel development. All design and implementation decisions must adhere to this principle.

**Core Requirements**:
- **100% POSIX Compatible**: Full compliance with POSIX standards
- **Linux ABI Fully Compatible**: Binary compatible with Linux kernel ABI
- **System Call Compatible**: Use Linux system call numbers
- **Filesystem Compatible**: Support Linux filesystems (ext4, btrfs)
- **ELF Format Compatible**: Executable format identical to Linux

**Design Philosophy**:
- **External interfaces must be 100% compatible** with Linux
- **Internal implementation can be improved** when it doesn't affect external compatibility

**Reference Resources**:
- Linux man pages (`man 2 syscall`) - for interface specifications
- POSIX standard: https://pubs.opengroup.org/onlinepubs/9699919799/ - for API behaviors
- Linux kernel source: https://elixir.bootlin.com/linux/latest/source - for understanding interface behaviors only

> **Key**: Our goal is to build a Linux-compatible OS kernel in Rust. External interfaces must be identical to Linux. Internal implementation is completely free - use the best design possible.

---

## ⚠️ Communication Style (user requirement)

When reporting to the user, **write like a normal engineer talking to a
colleague**:

- Plain language. No dramatic metaphors or marketing tone ("fleet",
  "full complement", "ignition", "salvage", "caught the culprit",
  "final hurdle" — all banned).
- State facts directly: what was fixed, what the root cause was, how it
  was verified, what is still broken.
- Call background tasks "background tasks" or by their plain name — no
  codenames the user then has to cross-reference.
- Reports need concrete numbers and conclusions ("LTP passes went from
  801 to 850"), not vague adjectives ("historic breakthrough",
  "major victory").
- The repository is English-only: commit messages, comments, docs, and
  code all in English (Chinese is fine in conversation with the user).

---

## Current Goals (2026-10)

Two active goals, in priority order:

1. **Boot a real GNOME desktop** on the Ubuntu 22.04 riscv64 image
   (`/home/william/rux-agents/gnome-img/gnome.img`, ignition driver `fire6.py`
   in the same directory). Status: Xorg + xterm + D-Bus work; gnome-session
   spawns components; screen lights up. Remaining blockers under active work:
   fake OOM kills (page allocator reports OOM at ~342MB/2G), and the
   component chain reaching gsd → gnome-shell.
2. **Maximize LTP pass rate** on the 1,869-test list. Trend so far:
   35 → 524 → 659 → 677 → 602 → 721 → 771 → 673 → 801 → 795.
   Full-scan results live in `/home/william/rux-agents/ltp-r<N>/results/`.

The rest of this file describes how to work toward those goals without
stepping on concurrent agents.

## Project Overview

**Rux** is a Linux-like operating system kernel entirely written in Rust.

- **Language**: Rust (no_std, except for necessary platform assembly)
- **Architecture**: **RISC-V only (riscv64)**
- **Platform**: QEMU `virt` machine, TCG with MTTCG (`thread=multi`) by default
- **Userspace**: Ubuntu 22.04 riscv64 rootfs (glibc), plus a musl-based
  minimal rootfs from `make rootfs`
- **Detailed status**: `docs/progress/roadmap.md` (kept up to date per milestone)

## Common Development Tasks

```bash
make build           # kernel only
make user            # userspace for the minimal rootfs
make rootfs          # minimal (musl) rootfs image
make run             # boot minimal rootfs shell
make gui             # boot the Ubuntu GUI image (udesk desktop)
make ubuntu-image    # rebuild the Ubuntu GUI test image
make ubuntu-run      # build + boot Ubuntu GUI image
make test            # in-kernel unit tests
```

- Default QEMU: `-M virt -accel tcg,thread=$(THREAD) -cpu rv64 -m 2G -smp 4`.
  `THREAD=single` restores single-threaded TCG for deterministic debugging.
- Never hand-write the qemu command for the standard flows; the Makefile
  carries required parameters. For custom runs, copy from an existing
  driver (e.g. `rux-agents/soak-24h/soak.py`).

## Key Files

- **Kernel.toml** - main kernel configuration (compiled into `kernel/src/config.rs` by build.rs; do not edit the generated file)
- **kernel/src/arch/riscv64/trap.rs** - trap/panic printer (KERNPANIC report + kernel-stack dump)
- **kernel/src/drivers/virtio/** - virtio-blk PCI/MMIO, async completions, descriptor windows (see `docs/architecture/lock-ordering.md` before touching lock nesting here)
- **kernel/src/fs/ext4/**, **kernel/src/fs/bio.rs** - filesystem + block layer (batch async reads, page cache)
- **test/** - userspace probes (compile with the riscv64 toolchain in `toolchain/`)
- **work/ubuntu-gui.img** - shared Ubuntu GUI test image (always boot with `-snapshot` + `file.locking=off`)

## Parallel-Agent Workflow (how this repo is actually developed)

Work happens in per-task worktrees under `/home/william/rux-agents/<name>/`
(130+ exist). Each background agent:

1. Creates/uses its own worktree, builds its own kernel there.
2. Commits fixes on a branch with **English commit messages**.
3. The main session reviews, cherry-picks into `main`, builds, smoke-boots,
   and pushes — only after the smoke boot reaches the login screen.
4. Merges that touch the same files (e.g. the virtio completion path, which
   has been reworked more than once) need semantic reconciliation, not
   mechanical conflict resolution.

**Concurrency: default to 3 background agents, not more.** Five burns the
API quota before the work finishes; three survives a full workday. Pick the
three highest-value tasks and queue the rest.

**Resource discipline (mandatory — the host runs many QEMUs at once):**

- Boot shared images with `-snapshot` and `file.locking=off` only.
- Each agent kills only QEMU PIDs it started; `pkill qemu` is forbidden.
- Default to `-smp 2 -m 1G` for test runs; SMP4/2G only for final validation.
- Long scans run detached (`setsid`) so tool timeouts don't kill them.
- Host memory is often near capacity: if QEMU fails to start, wait and
  retry instead of piling on.

**Known-flaky vs real regressions:** when a full LTP scan shows new
failures, rerun those cases in isolation (2x) before treating them as
regressions — host load produces timeout/precision flakes (observed:
accept02, getrusage04, kill02, genfmod, rename/statfs families).

## Debugging Aids

- Serial console output is the primary log (`console=ttyS0`).
- `dfx=taskdump,watchdog` boot flags enable periodic task dumps and the
  spinlock/deadlock watchdog; `dfx=memwatch` adds heap/page accounting.
- On kernel panic the trap printer emits epc/ra/sp/badaddr plus 24 stack
  words — symbolize with `riscv64-linux-gnu-addr2line -e <kernel> <addr>`.
- `debugfs` on a copied disk image extracts guest-side logs without booting.
- ext4 read/write coherence, virtio completion ordering, and wake-path
  lifetimes are historically bug-dense: read the R-numbered comments in
  the code before changing them.

## 📚 Documentation

- **[Getting Started](docs/guides/getting-started.md)** - up and running
- **[Roadmap](docs/progress/roadmap.md)** - milestone status and history
- **[Development Workflow](docs/guides/development.md)** - code standards
- **[Lock Ordering](docs/architecture/lock-ordering.md)** - in-kernel lock invariants (INV-LOCK-*)
- **[RISC-V Architecture](docs/architecture/riscv64.md)** / **[Boot](docs/architecture/boot.md)** / **[Memory](docs/architecture/memory.md)**
- **[Changelog](docs/progress/changelog.md)** / **[Quick Reference](docs/progress/quickref.md)**
