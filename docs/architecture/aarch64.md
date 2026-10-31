# AArch64 Architecture (QEMU virt, cortex-a72)

Status: stages 0-3 complete (interactive root shell, signals, full
in-kernel suite). Branch history carries the evidence for everything
below; this file is the operating map.

## Boot protocol (firmware-less ELF)

QEMU's `-kernel` with a plain ELF follows no image protocol on this
architecture: no DTB pointer in x0, `-append` and `-dtb` are ignored.
The kernel therefore embeds a full QEMU-virt DTB blob in `.rodata`
(symbol `aarch64_virt_dtb`, wired by `kernel/build.rs`); when the file
is absent (fresh clones — `*.dtb` is gitignored) build.rs dumps it from
the installed `qemu-system-aarch64 -M virt,gic-version=3 -cpu
cortex-a72 -m 1G -smp 2`. The cmdline comes from the blob's
`/chosen/bootargs`, else `DEFAULT_CMDLINE`.

**`-machine gic-version=3` is required on every boot** — the default
virt machine is GICv2 and both the GIC driver and the embedded DTB are
v3.

Entry: EL2 → drop to EL1h in `boot/boot.S` (HCR_EL2 = RW only: all
interrupt routing stays at EL1; CNTHCTL opens both timers; CPACR_EL1
FPEN is set — NEON traps otherwise).

## Memory map (constants in `arch/aarch64/mm/memory_layout.rs`)

- Linear map: phys P at `PAGE_OFFSET + P` (`PAGE_OFFSET =
  0xffff_c000_0000_0000`), **TASK_SIZE = 256 GB** — every consumer
  assumes user VAs fit inside TTBR0 L0 entry 0. (A 1 TiB constant once
  pushed the mmap base to 960 GB, outside the walker's subtree; the
  first mmap's TLS block then faulted forever.)
- Kernel image: `0xffff_ffff_8008_0000` (phys RAM base + arm64
  text_offset), 32 MB window.
- Kernel-half MMIO alias at `VMALLOC_START`: low window covers phys
  [0, 1 GB) (every virt MMIO device + the 32-bit PCI BAR window at
  0x1000_0000); a second window maps the **PCI ECAM at
  phys 0x40_1000_0000** (256 MB — high, fixed across RAM sizes).
  `mmio_alias()` routes ECAM-range addresses through the second window.
- **TCR_EL1.IPS = 44-bit** (`boot.S`): with the 32-bit default every
  translation whose output address ≥ 4 GB takes a level-2
  *address-size* abort (DFSC=2 — not a translation fault; the
  distinction cost a debugging day).
- The early TTBR0 identity window (phys [0, 2 GB)) built by boot.S is
  torn down after SMP start (`arch::mm::teardown_early_identity_map`,
  called from `rust_main_tail` right after `start_secondaries` — the
  secondary stubs are the last physical-PC consumers, and
  `secondary_cpu_entry` installs the high-VA VBAR *before* reporting
  itself started, so cpu_started(n) ⇒ core n needs no low VAs).
  Low-address kernel usercopies fault into the exception-table fixup →
  -EFAULT like on riscv64/x86_64; the boot core's TTBR0 parks on the
  kernel root (kernel-thread parity) and a broadcast `vmalle1is`
  clears every core's cached identity translations.

## Interrupts (GICv3)

`drivers/intc/gicv3.rs`: dist 0x0800_0000, redist 0x080A_0000 (both via
the MMIO alias), CPU interface by system registers. Notes that bit us:

- **GICD_CTLR under DS=1** (no security, QEMU): bit0 = Grp0 enable,
  **bit1 = Grp1NS enable**, ARE is RAO/WI. Writing the secure-world
  encoding leaves Group-1 forwarding off: zero IRQ exceptions with
  everything else armed, timer PPI latched pending in GICR_ISPENDINGR.
- GICv3 register offsets differ from v2 where it hurt: IGROUPR at
  0x0080 (0x0020 is reserved), ICC_SGI1R field layout (TargetList
  [15:0], INTID [27:24]) — the v2 encodings silently target nothing.
- Timer: virtual timer (CNTV_CVAL/CNTV_CTL), PPI 11 = INTID 27.
- PL011 RX: SPI 1 = INTID 33; IMSC needs RX **and** receive-timeout
  (bits 4|6) — a single typed char only raises the 32-cycle RT
  interrupt (FIFO threshold is 2).
- The unmask chain is two-legged and both legs are required:
  `enable_external_interrupt()` enables the GIC CPU interface *and*
  clears PSTATE.DAIF.I (the csrs-sie analogue — one without the other
  yields zero IRQ exceptions with no error anywhere).

## Per-task state the switch must carry

- **TPIDR_EL0 (user TLS)**: userspace writes it directly (no
  set-tls syscall on this ABI) — the kernel must treat it as task
  state: saved/installed at the `__switch_to` commit point, inherited
  by fork (glibc atfork handlers touch TLS before any child code
  runs), CLONE_SETTLS lands in the same slot, exec flushes both the
  slot and the live register. Losing this killed the first fork.
- **FPSIMD**: the eager v0-v31 switch exists, but prebuilt core
  libraries contain NEON (memcpy, fmt) that dirties user FP state
  mid-syscall — EL0 vector entries capture the live registers into
  `thread.user_fpsimd` and reinstall them before eret; the signal
  frame's fpsimd record is built from that buffer.
- TPIDR_EL1 remains the kernel's current-task pointer (the riscv `tp`
  protocol does not apply).

## Trap/syscall semantics that differ from the riscv64 twin

- **SVC leaves ELR at the instruction AFTER the svc** (ecall leaves
  SEPC at the ecall): never advance the PC on the syscall path; the
  ERESTARTSYS rewind is pc −= 4 onto the svc.
- Syscall number in x8, args x0-x5, return x0; `pt_regs.syscallno`
  slot + `orig_x0` for restarts.
- `uname`/auxv: AT_HWCAP advertises FP|ASIMD; the process-entry frame
  is pc/sp/pstate=EL0t with x0=0 (argc comes from [sp]).

## Devices

- virtio via PCI only (the virtio-mmio transports are unpopulated on
  this machine config): BARs are **self-assigned** from 0x1000_0000
  (no firmware ran; the DTB ranges window), modern caps, transitional
  device IDs on QEMU (net 0x1000, blk 0x1001).
- The virtio-blk probe's silent failure exits all print loudly
  (ECAM address + observable state) — a boot-time find/miss flake
  under host load was otherwise undiagnosable.

## Diagnostics on this port

- printk is **not routed** to the PL011 console path yet — only
  `print_status` table lines and `dfx::backtrace::ConsoleWriter` raw
  writes appear on serial; `pr_info` output is dropped.
- `dfx=` boot flags are unusable (the EL2 trampoline clobbers the DTB
  pointer x0 before cmdline parsing; the cmdline falls back to the
  default string) — compile-time features gate the probes instead.
- QEMU `-d int -D file` plus `aarch64-linux-gnu-addr2line -e <user
  binary>` and objdump around the faulting PC cracked every
  userspace-hang bug on this port; the toybox reference binary with
  symbols lives at `run/a64-userspace/toybox-0.8.13/` in the port
  worktree.

## Registered leftovers

Early identity-window teardown; printk→PL011 routing; kernel-spawned
init argv[0] uses the resolved path (workaround: name the init binary
so its basename is the applet name); a 1-of-9 boot flake with a trap
frame just past the linear-map end (kernel stack top question —
`aa64-fork/run/fix2.log`); LTP-on-aarch64 not started.
