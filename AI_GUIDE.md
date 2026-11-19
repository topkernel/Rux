# AI Guide — working on this repository

You are an AI agent about to modify this kernel. Read this file end to
end before your first change; it is the onboarding contract. Binding
policies (merge gate, parallel-agent workflow) live in
[AGENTS.md](AGENTS.md) — this file covers everything you need to get
productive; AGENTS.md covers what gets a change rejected.

## What this project is

A ~310,000-line operating-system kernel in Rust (`no_std`), ABI-compatible
with the reference kernel userspace contract: 345+ syscalls with matched
semantics, ELF/auxv, futex, SysV/POSIX IPC, binder, on-disk ext4, and the
`/proc` · `/sysfs` · `/debugfs` · tracefs layouts. Three architectures
from one source tree: **riscv64 (primary), x86_64, aarch64**. Unmodified
Ubuntu 22.04 and OpenHarmony images boot on it.

Two properties make this codebase unusual; respect both:

1. **The ABI is the contract, the internals are ours.** When in doubt,
   match the reference syscall semantics exactly (consult POSIX and the
   syscall man pages). Deliberate internal divergences are kept only with
   evidence and registered in
   [docs/architecture/design-divergences.md](docs/architecture/design-divergences.md).
2. **Every `unsafe` site carries a written safety contract**, and core
   unsafe code is proven for all inputs by Kani harnesses in
   `kernel/verify/`. If you touch code that has a mirror in
   `kernel/verify/`, you must update the mirror in the same change —
   `scripts/verify_sync_check.py` enforces it and `make verify` runs it.

## Environment

```bash
rustc --version                       # stable Rust
rustup target add riscv64gc-unknown-none-elf riscv64gc-unknown-linux-musl
rustup target add x86_64-unknown-none x86_64-unknown-linux-musl      # x86_64 work
rustup target add aarch64-unknown-none aarch64-unknown-linux-musl   # aarch64 work
qemu-system-riscv64 --version         # QEMU >= 5.0
qemu-system-x86_64 --version          # x86_64 work (KVM preferred)
riscv64-linux-gnu-gcc --version       # Ubuntu-image userland builds
```

KVM (`/dev/kvm`) is strongly recommended for x86_64 boots; riscv64 runs
under TCG with MTTCG by default.

## Build and run map

| Command | What it does |
|---|---|
| `make build` | riscv64 kernel (`target/riscv64gc-unknown-none-elf/debug/rux`) |
| `make build-x86` | x86_64 kernel (`target/x86_64-unknown-none/debug/bzImage`) |
| `make build PLATFORM=aarch64` | aarch64 kernel |
| `make sdk` | musl libc SDK for guest userland (`toolchain/build-musl.sh`) |
| `make user` | guest userland programs (runs `sdk` first, auto-restores C probes) |
| `make rootfs` | minimal musl rootfs image |
| `make run` | boot the minimal rootfs to an interactive shell |
| `make test` | in-kernel unit tests under QEMU (1,122 riscv64 · 1,119 aarch64) |
| `make verify` | mirror-sync check + property tests |
| `make kani` | Kani proofs (157 harnesses) |
| `make ubuntu-image` / `make ubuntu-run` | Ubuntu 22.04 GUI image build / boot |
| `./run/perf-gate.sh --tiers 1` | performance gate (see AGENTS.md before merging) |

Notes:

- `Kernel.toml` is compiled into `kernel/src/config.rs` by `build.rs`.
  Never edit the generated file.
- C probe sources are **not git-tracked**; they ship in
  `test/probes.tar.gz` and the x86_64 musl sysroot in
  `toolchain/x86_64-rux-linux-musl.tar.gz`. `scripts/extract-probes.sh`
  restores them on demand and is hooked into `make user` / `make test`.
  Compile probes with `toolchain/x86_64-rux-linux-musl/bin/musl-gcc`
  (x86_64) or `riscv64-linux-gnu-gcc` (riscv64).

## The verification ladder (run it in this order)

1. `make build` — clean compile, no warnings introduced.
2. `make test` — in-kernel unit tests. If you added a kernel feature,
   add a test to `kernel/src/tests/` (the kernel crate's `#[cfg(test)]`
   cannot run on the host — it collides with the std panic handler).
3. **Boot smoke** — `make run` reaches an interactive shell; for syscall
   changes, write or extend a C probe under `test/` and run it inside
   the guest. Probes are the A/B evidence standard here: reproduce the
   defect pre-fix, show it gone post-fix.
4. `make verify` — mirror sync + property tests. Red mirror = you
   changed kernel semantics without updating the proof.
5. `make kani` — required when you touched code covered by a harness.
6. Performance gate — read the tier list in
   [AGENTS.md](AGENTS.md); `run/perf-gate.sh --tiers 1` for every
   change, deeper tiers for hot-path changes. If a change affects
   performance through a path no tool measures, **add the DFX tool
   that measures it** — measuring is part of the change.

## Repository layout

```
kernel/src/            the kernel
  arch/<arch>/         per-arch boot, trap, mm glue (riscv64 is primary)
  sched/ mm/ fs/ ipc/  scheduler, memory, filesystems, IPC (incl. binder)
  drivers/             virtio, DRM, serial, …
  syscall/             syscall dispatch and implementations
  dfx/                 observability (kallsyms, debugfs, taskdump, watchdog)
  tests/               in-kernel unit tests (run under QEMU via make test)
kernel/verify/         Kani/proptest mirrors of kernel code — keep in sync
test/                  guest-side C probes + boot/test drivers (tar-packed)
userspace/             guest userland (musl world, toybox, LTP)
docs/                  architecture, guides, progress; keep updated
run/                   lane-local scratch and boot drivers (never commit
                       images or binaries)
```

## Conventions that get changes rejected

- Commit messages, code comments, and docs in **English**.
- Read the R-numbered comments (R1, R2, …) before changing historically
  bug-dense areas: ext4 read/write coherence, virtio completion
  ordering, wake-path lifetimes, and the lock nesting in
  [docs/architecture/lock-ordering.md](docs/architecture/lock-ordering.md)
  (INV-LOCK-* invariants are binding).
- Adding a field to a task/proc structure? Fill **both** construction
  paths (the struct literal and the `ptr::write(offset_of!)` path), or
  the field reads as allocator garbage.
- Register real behavior changes in
  [docs/progress/changelog.md](docs/progress/changelog.md).

## Debugging toolkit

- The serial console (`console=ttyS0`) is the primary log.
- `dfx=taskdump,watchdog` boot flag: periodic task dumps + spinlock
  watchdog. `dfx=memwatch` adds heap/page accounting.
- On panic the trap printer emits epc/ra/sp/badaddr plus 24 stack words;
  symbolize with
  `riscv64-linux-gnu-addr2line -e <kernel-elf> <addr>`.
- Boot shared disk images **only** with `-snapshot` + `file.locking=off`.
- When a full test scan shows new failures, rerun them in isolation
  (2×) before treating them as regressions — host load produces
  timeout flakes.

## Working alongside other agents

Multiple agents work in this repository concurrently. The rules in
[AGENTS.md](AGENTS.md) are binding: do your work in your own worktree
under a task directory, never run `pkill` against QEMU, kill only
processes you started, and never commit disk images or scratch
binaries. Main-branch integration (build + smoke boot + gate) happens
through the coordinator, not directly.
