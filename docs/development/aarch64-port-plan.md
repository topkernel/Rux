# aarch64 Port Plan (living spine)

> Status header updated at the end of every session.
> Owner: aarch64 bring-up lane (worktree `/home/william/rux-agents/aarch64-port`,
> branch `feature/aarch64-port`).
> Companion doc: `docs/development/multiarch-gnome-ubuntu-plan.md` §3 (the
> original A1–A8 sketch; this doc supersedes and extends it for the standing
> aarch64 lane).

## Status

| Stage | Content | State |
|---|---|---|
| 0 | Plan, module map, seam audit, toolchain | done 2026-10-29 |
| 1 | Serial-first bring-up (PL011 banner + early mm) | done 2026-10-29 |
| 2 | Traps + context switch + `make test` on aarch64 | pending |
| 3 | SMP (PSCI) + GICv3 + virtio-mmio + user shell | pending |
| 4 | Triple-arch gate integration (coordinator) | pending |

## 1. Target and conventions

Same "virt machine" convention as riscv64:

- QEMU `-M virt -cpu cortex-a72 -accel tcg` (resource discipline: `-smp 2 -m 1G` class)
- UART: PL011 at `0x0900_0000` (console.rs already has an `#[cfg(feature="aarch64")]` putc arm)
- Interrupts: GICv3 (`-machine gic-version=3` is the default with cortex-a72 on recent QEMU; verify `gic-version` in the generated DTB at bring-up)
- Timer: architected generic timer (CNTFRQ_EL0 / CNTVCT_EL0 / CNTPS/CNTP EL1 timers), NOT MMIO CLINT
- SMP boot: PSCI 0.2 via HVC conduit (QEMU virt exposes PSCI in the DTB; `-kernel` direct boot starts all CPUs parked in PSCI `cpu_on` wait... verify: QEMU direct Linux boot starts secondary CPUs either spinning on a pen or via PSCI depending on `-smp`; with PSCI in DTB, use CPU_ON)
- Devices: virtio-mmio (the virt machine's default transports at 0x0a00_0000+; no PCI dependency for stage 3)
- Boot protocol: QEMU `-kernel` ELF direct boot. QEMU loads ELF segments at their
  physical addresses and enters at `e_entry` with `x0` = DTB pointer, at EL2
  (highest non-secure EL on virt). Kernel drops to EL1.

### Memory map decisions (stage 1)

| Item | Value | Note |
|---|---|---|
| Kernel load PA | `0x4008_0000` | QEMU virt RAM base 0x40000000 + 0x80000 (Linux arm64 `text_offset` convention); stays clear of the DTB QEMU places mid-RAM |
| Kernel link VA | `0xffff_8000_0000_0000` region | TTBR1 upper half, 48-bit VA, 4K granule, 4-level tables; mirrors how riscv64 links at `0xffff_ffff_8000_0000` in its Sv39 upper half |
| Entry section | `.text.entry` linked at the PHYSICAL address | same trick as riscv64: `_start` runs MMU-off at its LMA; rest of image at high VMA with `AT()` LMAs |
| User VA | TTBR0 lower half, `USER_STACK_TOP` per existing config | keep the riscv64-style user layout so generic mm code sees the same shapes |

### Target triple

`aarch64-unknown-none` (already wired in `.cargo/config.toml`, hard-float aapcs64).
FPSIMD: the compiler may emit FP for kernel code only if it sees float types;
kernel code is integer-only today (same policy as the other two backends). The
real FPSIMD context switch (eager save of FPCR/FPSR + 32 Q regs, or lazy
trap-on-FPSIMD-trap) is a stage-3 work item before any float-heavy userspace
(the multiarch doc's warning: do not repeat the riscv64 ARCH-H1 "save but never
restore" lesson).

## 2. Toolchain (installed 2026-10-29, stage-0 homework — done)

- `rustup target add aarch64-unknown-none` (via rsproxy mirror; nightly 1.101.0)
- `qemu-system-aarch64` 10.2.1 (Debian, `apt install qemu-system-arm`)
- `aarch64-linux-gnu-gcc` 15.2.0 (boot.S assembler, later userspace probes)

## 3. Build wiring inventory (what stage 1 touches)

| File | Change |
|---|---|
| `build/Makefile` | `aarch64` branch: `CARGO_TARGET=aarch64-unknown-none`, `CARGO_FEATURE_FLAGS=--no-default-features --features aarch64`; `build-aarch64` alias |
| `build/current-platform.sh` | accept `aarch64` in override + case lists |
| `kernel/build.rs` | (a) platform precedence: `CARGO_FEATURE_AARCH64` env alongside x86_64; (b) consistency-guard arms for aarch64; (c) `compile_boot_asm()` aarch64 branch calling `aarch64-linux-gnu-gcc -c`; (d) heap clamp: aarch64 joins the riscv64 ≤128 MiB clamp (unmeasured arch, 1G QEMU target) |
| `kernel/src/arch/mod.rs` | `pub use aarch64::{...}` re-export arm (the 16-item surface + `context_switch`) |
| `.cargo/config.toml` | already has `[target.aarch64-unknown-none]` with linker script path (file itself is new in stage 1) |
| `Kernel.toml` | `[platform]` values stay as-is; `PLATFORM=aarch64 make build` works via the override path (no `.config` flip needed) |
| `test/run-aarch64.sh` | new boot driver (worktree `run/` during bring-up; promote to `test/` when stable) |

Entry-point subtlety (from riscv64): boot.o must be linked FIRST so `_start`
sits at the head of the image; the riscv64 window lesson (LAYOUT-2/3/4 comments
in `arch/riscv64/linker.ld`) applies — the early boot page tables must map the
whole image with room to grow, and the linker script must ASSERT the image fits.

## 4. Module map (riscv64 → aarch64)

Line counts: riscv64 arch tree ~14.2k, x86_64 ~14.4k. Expected aarch64 effort
is the same order; the stub tree today is 234 lines (11 top modules + 9 mm
modules of `unimplemented!`), plus a real `pt_regs.rs` skeleton (127 lines).

| riscv64 module (lines) | aarch64 counterpart | Hardware mapping |
|---|---|---|
| `boot.S` + `boot.rs` (32) | `boot.S` + `boot.rs` | OpenSBI S-mode entry (a0=hart, a1=dtb) → QEMU EL2 entry (x0=dtb), EL2→EL1 drop, identity tables, MMU on, jump to high VA `rust_main` |
| `linker.ld` | `linker.ld` | phys 0x80200000 → 0x40080000; virt 0xffffffff80000000 → TTBR1 upper half; same `.text.entry`-at-LMA trick, `AT()` for the rest |
| `trap.S`/`trap.rs` (1833) | `trap.S`/`trap.rs` | `stvec` + `sscratch` two-phase entry → VBAR_EL1 16-slot vector table (4 types × current/el0 × a/sp), SPSR/ELR/ESR/FAR save, SVC #0 for syscalls (x8=nr, x0-x5 args, x0 ret) |
| `context.rs` (587) | `context.rs` | callee-saved s0-s11 → x19-x28, sp, lr (+ later FPSIMD) |
| `cpu.rs` (184) | `cpu.rs` | CSRs → system regs: `read_time`=rdtime → CNTVCT_EL0; `read_counter`→ PMCCNTR or CNTVCT; `get_counter_freq`→ CNTFRQ_EL0; wfi→wfi (same); irq save/restore → DAIF msr/mrs |
| `ipi.rs` (487) | `ipi.rs` | PLIC SGI (`msip`) → GICv3 SGI (SGI→IPI irq routing) |
| `mm/*` (7.5k) | `mm/*` | Sv39 3-level → AArch64 4-level (48-bit VA, 4K granule); satp→TTBR0/TTBR1; sfence.vma→`tlbi vmalle1`/`tlbi vae1is`; PTE bits V/R/W/U/A/D → valid + AttrIndx(MAIR) + AP/PXN/UXN + DBM/AF (software-maintained A/D like riscv) |
| `mm/memory_layout.rs` (566) | same | PAGE_OFFSET/VMALLOC/FIXMAP/window constants re-derived for the TTBR1 split |
| `mm/fixmap.rs` (267) | same | early UART PL011 map (riscv64 maps the 16550a MMIO) |
| `mm/asid.rs` (302) | same | satp.ASBits→ASID (8/16-bit per GICv3/TCR_EL1.A1S; QEMU 64K ASIDs ok), `tlbi aside1is` |
| `smp.rs` (345) | `smp.rs` | SBI HSM hart_start → PSCI `CPU_ON` (HVC conduit), `MPIDR_EL1` hashing for cpu index |
| `pt_regs.rs` (458) | `pt_regs.rs` (stub is real: 31 GPR + sp/pc/pstate/esr) | pt_regs layout must stay Linux-arm64-shaped for ptrace ABI |
| `process.rs` (325) | `process.rs` | copy_thread sets `pstate=EL0h` (vs riscv64 `sstatus` PPE); ret_from_fork |
| `thread.rs` (525) | `thread.rs` | ThreadStruct equivalent (task thread info), arch_prctl → arch-specific prctl stub |
| `uaccess.rs/.S` (451) | `uaccess.rs` | sum-check + PAN-style access_ok; aarch64 kernel runs with PAN if available (cortex-a72: PAN is ARMv8.1 — QEMU cortex-a72 does NOT implement PAN; rely on access_ok + exception-table fixups via ESR EL1h data abort) |
| `vext.rs` (386) | — (none) | riscv vector extension probe — no aarch64 equivalent needed (SVE later, not for bring-up) |
| — | `drivers/intc/gicv3.rs` | module already declared in `drivers/intc/mod.rs` behind `#[cfg(feature="aarch64")]`; file missing (create in stage 3) |
| CLINT (`drivers/intc/clint.rs`) | arch timer regs | no MMIO timer needed; CNTFRQ-based |

## 5. Interface contract (what arch/ must export)

Code outside `arch/` uses **193 unique `arch::` paths** (grep over all non-arch
`.rs`, counts = call sites). The aarch64 backend must provide all of them. Top
of the list by usage:

```
uaccess: access_ok(261) copy_to_user(129) copy_from_user(128) put_user(73)
         get_user(51) strncpy_from_user(11) clear_user(9) MAX_RW_COUNT(1)
cpu_id(87)                                    [mod.rs root fn]
cpu: restore_irq(72) read_time(48) read_counter(29) wfi(14) save_and_disable_irq(12)
     get_thread_id(11) get_interrupts_state(9) get_counter_freq(2) outb/inb/invlpg(2,x86-only)
mm: virt_to_phys(55) VirtAddr::new(54) phys_to_virt(22) PAGE_SHIFT(17)
    mmu_init::get_page_table_virt(23) mm_ops::PageTableWalker(16+7) pte_lock_for(8)
    memory_layout::{PhysAddr, phys_to_virt(14), PAGE_OFFSET, mmio_alias, device_alias}
    user_addr::{USER_END,USER_START,BRK_DEFAULT,TASK_SIZE,STACK_MAX_SIZE,STACK_TOP,BRK_MAX}
    mm_ops::{PTE_MODIFY_LOCK,MM_CTX, map_user_region(4), cow_flags::{COW,PCACHE}, ...}
    asid::{flush_tlb_all(5)} free_asid/alloc_asid/flush_tlb_page/read_satp/write_satp/build_satp
    pagetable::{PageTableEntry::{V,R,W,U,D,A,P,RW,DIRTY,ACCESSED,from_bits}, PageTable}
    PtLedger::live_count, mmap_error, VA_PA_OFFSET, mmu_init::{root_static_phys,
    root_page_table_ppn, map_kernel_region}, alloc_and_map_user_memory, ...
pt_regs::PtRegs(38) pt_regs::Cause(1) PT_REGS_SIZE (via backend paths, cfg-gated)
smp: num_started_cpus(14) cpu_id(9) PER_CPU(5) is_boot_hart cpu_started
thread::ThreadStruct(10) CalleeSaved(1)
trap: current_pt_regs(3) enable/disable_timer_interrupt request_irq_line
      pt_regs_rip_for_cpu
boot: get_dtb_pointer(5) boot_cmdline boot_memory_regions
context_switch (via arch::context_switch)
process: sys_arch_prctl(1) flush_thread(1)
```

Plus the module surface re-exported from `arch/mod.rs`:
`arch_init, cpu_id, enable_interrupts, init` (root fns) and modules
`boot, context, cpu, ipi, mm, process, pt_regs, smp, thread, trap, uaccess`.

The old gap report (multiarch doc §1: "1354 errors") was dominated by MISSING
RE-EXPORTS (`arch::cpu::cpu_id` 43 sites, timer `get_jiffies` 109 sites), not
by missing backend logic — wire the re-export arm early and the error count
collapses to the true backend gap.

## 6. Seam audit: riscv64 leaks OUTSIDE arch/ (as of dfa523ab)

Two leak classes exist today. Neither blocks an aarch64 build (both are
cfg-guarded), but both need `aarch64` arms added when the feature is enabled:

1. **Direct backend-path references** — 67 sites in 10 files, each inside
   `#[cfg(feature="x86_64")]` / `#[cfg(feature="riscv64")]` blocks (some have a
   `#[cfg(not(any(...)))]` fallback arm, some do not):
   - `sync/futex.rs` (27): pt_regs stack-walk diagnostics (PT_REGS_SIZE + regs field reads). Needs an aarch64 arm reading `sp/pc/pstate` or an interface fn `pt_regs_sp_of(stack_top)`.
   - `ipc/binder.rs` (22): same pattern, binder stall diagnostics.
   - `fs/vfs.rs` (4), `dfx/slowtask.rs` (4), `drivers/virtio/virtio_pci.rs` (3, x86-only anyway), `dfx/scribble.rs` (2), `dfx/memwatch.rs` (2), `syscall/process.rs` (1), `main.rs` (1: `arch::riscv64::vext::prime_hart_has_v` — riscv-gated already), `fs/ext4/file.rs` (1).
2. **`cfg(target_arch = ...)` blocks** — 25 sites in 5 files:
   - `process/task.rs` (11): the x86_64 80KB packed-slab stack slots (`cfg(x86_64)`-only — aarch64 correctly falls to the 128KB base already).
   - `syscall/memory.rs` (8): mprotect/mmap PTE-flag arms per arch.
   - `ipc/binder.rs` (3), `dfx/ksyms.rs` (2, symbol name mangling per target), `tests/config_test.rs` (1).

Policy for the port: add `#[cfg(feature="aarch64")]` arms ONLY where semantics
demand it (mprotect flag mapping, futex/binder pt_regs reads); where a
`not(any(...))` fallback exists, let aarch64 use the fallback until stage 3
needs better. Never de-cfg the riscv64/x86_64 arms.

Generic-layer wins already present (verify at each stage): `cmdline.rs`
(dtb-generic), memblock, `PageTableWalker`-based COW (the x86 fork fix
5c32dad7), exception table, futex/pipe/binder core.

## 7. Stage gates

Each stage ends with: **the OTHER two arches build rc=0** (riscv64 default
build + `PLATFORM=x86_64 make build`), **`make test` green on riscv64**
(1096/1096), and per-stage serial-log evidence from the aarch64 boot.

### Stage 0 — plan (this doc) + toolchain
Acceptance: doc committed; `aarch64-unknown-none` rustc target, qemu-system-aarch64, aarch64-linux-gnu-gcc installed. DONE.

### Stage 1 — serial-first bring-up (A1 of the old sketch)
Scope: build wiring (§3), `boot.S` (EL2→EL1, x0=dtb saved, BSS clear, early
identity→kernel map, MMU on via TCR/TTBR/MAIR, jump to high-VA `rust_main`),
`linker.ld`, `mm/memory_layout.rs` constants, PL011 early output, early
memblock from DTB (`/memory` node + cmdline reuse), `arch_init()` stubs that
let the generic banner print.
Acceptance: QEMU virt serial shows the Rux boot banner and reaches the first
`unimplemented!` in a controlled place (expected: trap/mm init); no QEMU
abort. Evidence: serial log.

### Stage 2 — traps + context switch + unit tests (A2/A3/A4)
Scope: VBAR_EL1 vector table + trap.S frame save; `handle_exception` routing
(sync/irq/SVC); `context.rs` context_switch; `process.rs` copy_thread +
ret_from_fork; `mm/` full pagetable/mmu_init/mm_ops/page_fault (the 7.5k-line
beast); uaccess; `make test` path (unit-test feature) on aarch64.
Acceptance: `PLATFORM=aarch64 make test` runs the in-kernel suite and prints
pass/fail counts; smoke riscv64+x86_64 still green.

### Stage 3 — SMP + IRQs + devices + user shell (A5/A6/A7 minimal)
Scope: `drivers/intc/gicv3.rs` (dist/redist/CPU-iface, SPI/PPI/SGI), arch
timer tick at 100Hz, PSCI CPU_ON secondaries + SGI IPI, virtio-mmio probe +
blk, ELF user exec at EL0, minimal rootfs → mrsh shell.
Acceptance: `make rootfs`-class image boots to `/bin/sh` prompt on aarch64,
`uname`-class smoke (LTP not in scope yet).

### Stage 4 — gates integration (coordinator)
Propose: merge gate row 1 becomes triple-arch build; `suite.sh` gains aarch64
legs; riscv64+x86_64 perf baselines untouched (aarch64 gets no perf gate rows
yet — DFX tools first, per the perf-gate mandate).

## 8. Risks / open questions

1. **QEMU EL2 vs EL1**: with `-cpu cortex-a72` QEMU virt normally enters at
   EL2. boot.S must detect CurrentEL and drop to EL1 cleanly (set SPSel=1,
   program SCTLR, HCR_EL2 to route IRQs/FIQs). Verify at stage-1 boot.
2. **GICv3 vs GICv2**: recent QEMU defaults `virt` to GICv3 for A72-class
   CPUs, but the DTB is authoritative — parse `/intc` compatible at stage 3.
3. **AF/DBM hardware flags**: QEMU cortex-a72 (ARMv8.0) has no hardware
   A/D (access flag / dirty) management — software A/D bits like riscv64
   (stage 2 mm design).
4. **FPSIMD context**: mandatory before any float userspace (stage 3);
   cortex-a72 has 32×128-bit Q regs + FPCR/FPSR.
5. **Image size window**: replicate the riscv64 LAYOUT-4 lesson — linker
   ASSERT + early window covering the full image including the 256KB boot
   stack and (later) the ksyms table.
6. **`get_jiffies` timer re-export**: the 109-site re-export comes from the
   timer module; confirm the aarch64 timer module provides the same surface
   (stage 2/3 — timer.rs is generic but reads `arch::cpu::read_time`).
