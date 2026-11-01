# Boot architecture (three ports)

*Refreshed 2026-11-01. This is the current shape of all three boot
paths; the April riscv64 deep-walk is archived (2026-04-riscv64-boot.md)
and remains valid where it overlaps.*

All three ports boot via QEMU direct-kernel-boot (`-kernel`), not a
firmware chain: the kernel image is loaded by the machine model and
entered at its arch entry. The common sequence in `rust_main()` is:
early arch bring-up (page tables, serial) → memory init (memblock,
zones, vmemmap, the small-object allocator) → traps/IRQs → scheduler
and init task → driver probe (virtio on the discovered transport) →
mount root → run init.

## riscv64 (primary: `virt`, OpenSBI)

- Entry: QEMU loads OpenSBI (fw_dynamic) in M-mode; it hands S-mode to
  the kernel at `_start` with `a0 = hartid`, `a1 = DTB pointer`, and
  parks other harts. Each hart sets `stvec` early, sets up `tp`
  (per-CPU area), builds the Sv39 kernel tables, then secondaries wait
  for the boot CPU's release.
- Serial `console=ttyS0` (NS16550a at DTB-described MMIO), `root=
  /dev/vda` on virtio-blk. The minimal rootfs boots to mrsh/toybox;
  the Ubuntu image boots the full desktop stack.

## x86_64 (q35)

- Entry: multiboot-loaded ELF → 64-bit stub sets stacks and calls
  `rust_main`; SMP via a trampoline (SIPI). The boot protocol work
  (bzImage) lets the same image boot where a Linux-boot protocol
  loader is expected.
- Early page tables: bootstrap PML4 with an identity low window for
  the boot stack; `mm::init()` builds the real higher-half kernel
  tables and switches CR3 — and the bootstrap identity link (PML4[0])
  is **unlinked** right after the stack switch (boot_stack_continuation):
  there is no identity MMIO window at runtime (D-17; device MMIO goes
  through the kernel-half alias arena).
- CR4.SMAP is enabled at arch init (D-18); usercopy is AC-bracketed.
- Serial `console=ttyS0`, virtio on PCI (BARs resolved through the
  alias arena; above-4G BARs ioremapped).

## aarch64 (QEMU `virt`, GICv3)

- Entry: `-kernel` image at the loader address with DTB in x0/dtb
  pointer preserved through the vector-table setup (a fixed boot bug
  family); EL1, per-CPU areas, Sv39-equivalent 4-level tables.
- `make build PLATFORM=aarch64`, boot with `-M virt,gic-version=3`
  (see [aarch64.md](aarch64.md) before touching the port).

## Boot-time invariants worth knowing before editing

- The per-arch early code runs before the allocator exists: static
  BSS pools only (the stage design is documented in mmu_init headers).
- The boot CPU owns all early initialization; secondaries spin on a
  release flag until the scheduler exists.
- `init=` resolution and the PID-1 contract (including script-init via
  the rootfs shell) happen at the end of `rust_main`; the kernel test
  harness and benchmark runners (LTP ltpscan, lmbench) are themselves
  PID-1 ELFs.
