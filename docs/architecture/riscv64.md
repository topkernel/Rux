# RISC-V 64-bit Architecture Implementation

This document describes RISC-V-specific implementation details in the Rux kernel.
For boot process, memory layout, and page table management, see their dedicated documents.

**Last Updated**: 2026-03-27

---

## Table of Contents

- [Target Platform](#target-platform)
- [RISC-V Extensions Used](#riscv-extensions-used)
- [CSR Register Usage](#csr-register-usage)
- [PtRegs Structure](#ptregs-structure)
- [Trap Entry/Exit Mechanism](#trap-entryexit-mechanism)
- [Context Switch](#context-switch)
- [FPU State Management](#fpu-state-management)
- [User Memory Access](#user-memory-access)
- [SBI Interface](#sbi-interface)
- [ASID and TLB Management](#asid-and-tlb-management)
- [SMP and IPI](#smp-and-ipi)
- [Memory Ordering](#memory-ordering)
- [RISC-V Instructions Reference](#riscv-instructions-reference)
- [References](#references)

---

## Target Platform

| Property | Value |
|----------|-------|
| Architecture | RV64GC (I M A F D C) |
| MMU | Sv39 (3-level page table) |
| Privilege | S-mode (supervisor), with OpenSBI in M-mode |
| CPUs | 4 cores (QEMU virt platform) |
| Extensions | SSTC (timer), Sstc (stimecmp) |

**Privilege Model**:
```
M-mode: OpenSBI firmware (boot, SBI calls)
S-mode: Rux kernel
U-mode: User applications (shell, toybox, etc.)
```

## RISC-V Extensions Used

| Extension | Name | Usage |
|-----------|------|-------|
| I | Integer | Base instruction set |
| M | Multiply/Divide | `mul`, `div`, `rem` |
| A | Atomic | `amoswap` (kernel big lock), `lr/sc` |
| F | Float | FPU state save/restore |
| D | Double-precision Float | 64-bit float registers |
| C | Compressed | 16-bit instructions (ecall, c.addi, etc.) |
| Sv39 | Page Table | 3-level virtual memory |
| Sstc | Supervisor Timer | `stimecmp` CSR for timer interrupts |

---

## CSR Register Usage

The kernel uses the following S-mode CSRs:

| CSR | Access | Purpose |
|-----|--------|---------|
| `stvec` | R/W | Trap vector base address |
| `sscratch` | R/W | Trap source detection (swapped with tp) |
| `sstatus` | R/W | SPP, SPIE, SIE, SUM, FS fields |
| `sepc` | R/W | Exception return address |
| `stval` | R | Fault address (badaddr) |
| `scause` | R | Exception/interrupt cause |
| `satp` | R/W | Page table root + ASID |
| `sie` | R/W | Interrupt enable (STIE, SSIE, SEIE) |
| `sip` | R/W | Interrupt pending bits |
| `stimecmp` | R/W | Timer compare (SSTC extension) |

Key `sstatus` fields:

| Bit(s) | Field | Purpose |
|--------|-------|---------|
| 1 | SIE | Supervisor Interrupt Enable |
| 5 | SPIE | Supervisor Previous Interrupt Enable |
| 8 | SPP | Supervisor Previous Privilege (0=U, 1=S) |
| 13:14 | FS | FPU state (OFF=0, INITIAL=1, CLEAN=2, DIRTY=3) |
| 18 | SUM | Supervisor User Memory access |

M-mode CSRs readable from S-mode (via OpenSBI):

| CSR | Purpose |
|-----|---------|
| `mhartid` | Hardware thread ID (used during early boot) |
| `mimpid` | Implementation ID |
| `marchid` | Architecture ID |

---

## PtRegs Structure

`PtRegs` is saved on the kernel stack at every trap entry. It holds all CPU state needed to restore the interrupted context.

**File**: [pt_regs.rs](../../kernel/src/arch/riscv64/pt_regs.rs) (288 bytes, `#[repr(C)]`)

```
Offset  Field       Source        Description
------  -----       ------        -----------
0x000   epc         sepc          Exception program counter
0x008   ra          x1            Return address
0x010   sp          x2            Stack pointer
0x018   gp          x3            Global pointer
0x020   tp          x4            Thread pointer
0x028   t0          x5            Temporary 0
0x030   t1          x6            Temporary 1
0x038   t2          x7            Temporary 2
0x040   s0          x8            Saved 0 (frame pointer)
0x048   s1          x9            Saved 1
0x050   a0          x10           Argument 0 / return value
0x058   a1          x11           Argument 1
0x060   a2          x12           Argument 2
0x068   a3          x13           Argument 3
0x070   a4          x14           Argument 4
0x078   a5          x15           Argument 5
0x080   a6          x16           Argument 6
0x088   a7          x17           Syscall number
0x090   s2          x18           Saved 2
0x098   s3          x19           Saved 3
0x0A0   s4          x20           Saved 4
0x0A8   s5          x21           Saved 5
0x0B0   s6          x22           Saved 6
0x0B8   s7          x23           Saved 7
0x0C0   s8          x24           Saved 8
0x0C8   s9          x25           Saved 9
0x0D0   s10         x26           Saved 10
0x0D8   s11         x27           Saved 11
0x0E0   t3          x28           Temporary 3
0x0E8   t4          x29           Temporary 4
0x0F0   t5          x30           Temporary 5
0x0F8   t6          x31           Temporary 6
0x100   status      sstatus       Supervisor status
0x108   badaddr     stval         Fault address
0x110   cause       scause        Exception cause
0x118   orig_a0     --            Original a0 (for syscall rollback)
```

**Design notes**:
- Register ordering follows RISC-V ABI (x1 through x31 sequentially)
- `orig_a0` is separate from `a0` to support syscall rollback (Linux convention)
- `a0` may be overwritten by the handler; `orig_a0` preserves the original value
- `cause` parsing: MSB=1 means interrupt, MSB=0 means exception
- `user_mode()` checks `sstatus.SPP == 0`

---

## Trap Entry/Exit Mechanism

**Files**: [trap.S](../../kernel/src/arch/riscv64/trap.S), [trap.rs](../../kernel/src/arch/riscv64/trap.rs)

### Trap Source Detection: sscratch/tp Protocol

The kernel uses the sscratch/tp swap to detect whether a trap came from user or kernel mode:

| Running State | sscratch | tp |
|---------------|----------|----|
| Kernel | 0 | current task pointer |
| User | current task pointer | user TLS |

```asm
trap_entry:
    csrrw tp, sscratch, tp    # Atomic swap tp and sscratch
    bnez  tp, .Lfrom_user     # tp != 0 → came from user mode
    j     .Lfrom_kernel       # tp == 0 → came from kernel mode
```

During early boot (before task_struct exists), tp is the hart_id directly (< 0x1000). The code detects this and falls back to checking `sstatus.SPP`.

### Kernel Trap Path

1. Restore tp from sscratch (get current task pointer back)
2. Select per-CPU interrupt stack (based on `ti_cpu` field at task_struct+0x18)
3. Skip if already on interrupt stack (nested trap)
4. Allocate PtRegs (288 bytes) on stack
5. Save all 31 GPRs
6. **Read CSRs after registers** (CSR reads clobber t0-t3)
7. Call `trap_handler(regs)`

### User Trap Path

1. Save user sp, load kernel sp from `task_struct.ti_kernel_sp`
2. Allocate PtRegs on kernel stack
3. Save all 31 GPRs and user tp
4. Read CSRs
5. **Acquire kernel big lock** via `amoswap.d.aq`
6. Clear sscratch to 0 (mark: now in kernel)
7. Call `trap_handler(regs)`

### Trap Exit to User

1. Save `ti_kernel_sp` (unwound kernel stack pointer)
2. **Signal/reschedule loop**: check pending signals → check need_resched → call schedule() if needed → reload sp → loop
3. Restore sstatus and sepc
4. Restore all GPRs
5. **Release kernel big lock** via `amoswap.d.rl`
6. **Clear LR/SC reservation**: `sc.d x0, t2, (t2)` (prevents cross-context reservation leakage)
7. Set sscratch = tp (current task), restore user tp
8. `fence iorw, iorw` (memory barrier)
9. Restore user sp, `sret`

### ret_from_fork Paths

When a forked child or kernel thread first runs, it enters via these paths:

- `ret_from_fork_user_asm`: calls `schedule_tail(prev)`, then `ret_from_fork_user(regs)`, then falls through to `ret_from_exception` (user return path)
- `ret_from_fork_kernel_asm`: calls `schedule_tail(prev)`, then `ret_from_fork_kernel(fn_arg, fn_ptr, regs)`, then falls through to `ret_from_exception`

### Task Structure Offsets (hardcoded in assembly)

```
Offset  Field
------  -----
0x00    TASK_TI_FLAGS
0x04    TASK_TI_PREEMPT
0x08    TASK_TI_KERNEL_SP
0x10    TASK_TI_USER_SP
0x18    TASK_TI_CPU
0x1C    TASK_STATE
0x20    TASK_PID
```

---

## Context Switch

**File**: [context.rs](../../kernel/src/arch/riscv64/context.rs)

### `context_switch(prev, next)` (Linux-style order)

```
1. fpu_save_for_switch()        # Save prev FPU state
2. set_prev_task(prev)          # Store prev for ret_from_fork
3. switch_mm(next_ppn)          # Write SATP if address space changed
4. __switch_to(prev, next)      # Switch registers (tp updated here)
5. restore_fpu()                # Restore next FPU (via tp → current task)
```

### `__switch_to` (inline assembly)

Saves and restores callee-saved registers between two tasks:

| Save (prev) | Restore (next) |
|-------------|----------------|
| ra (x1) | -- (returns via `ret`, not `sret`) |
| sp (x2) | sp (x2) |
| s0-s11 (x8-x9, x18-x27) | s0-s11 |
| sstatus.SUM | sstatus.SUM |

After saving prev and restoring next, sets `tp = next` (task pointer). This is how the kernel tracks the current task — `tp` always points to the current `Task`.

### `switch_mm(next_ppn)`

```asm
sfence.vma zero, zero        # Flush TLB before switch
csrw      satp, {satp}      # Write new page table (Sv39 mode=8, ASID=0)
sfence.vma zero, zero        # Flush TLB after switch
```

The function is placed in linear mapping region (VPN2 >= 256) so it remains accessible after the SATP change.

---

## FPU State Management

**File**: [thread.rs](../../kernel/src/arch/riscv64/thread.rs)

### ThreadStruct FPU Fields

```
Field       Type       Description
-----       ----       -----------
fpu         [u64; 32]  FPU registers f0-f31 (64-bit each)
fcsr        u32        FPU control/status register (rounding mode, exceptions)
fs          u32        Saved sstatus.FS field
```

### Lazy FPU State Machine (Linux-style)

The `sstatus.FS` field controls FPU access:

| Value | State | Meaning |
|-------|-------|---------|
| 0 | OFF | FPU disabled, no state to save |
| 1 | INITIAL | FPU initialized but not yet used |
| 2 | CLEAN | FPU state saved, registers match memory |
| 3 | DIRTY | FPU modified since last save |

### FPU Operations in Context Switch

**`fpu_save_for_switch()`** (called before `__switch_to`):
1. Read `sstatus.FS`
2. If DIRTY: save f0-f31 via `fsd`, save fcsr via `frcsr`
3. Clear FS to OFF (disables FPU traps for next context)

**`restore_fpu()`** (called after `__switch_to`, uses tp for current task):
1. If fs != OFF: enable FPU via `csrs sstatus, SR_FS`
2. Restore fcsr via `fscsr`, restore f0-f31 via `fld`
3. Set FS to CLEAN

**`fpu_init()`**: Sets FS=INITIAL, zeros all f0-f31 via `fcvt.d.l fN, zero`, zeros fcsr.

### FPU Instructions Used

```
fsd  fN, offset(base)    # Store 64-bit float register
fld  fN, offset(base)    # Load 64-bit float register
frcsr rd                 # Read FPU control/status
fscsr rs                 # Write FPU control/status
fcvt.d.l fN, zero        # Zero a float register (convert int 0 to double)
csrs sstatus, SR_FS      # Enable FPU
csrc sstatus, SR_FS      # Disable FPU
```

---

## User Memory Access

**Files**: [uaccess.rs](../../kernel/src/arch/riscv64/uaccess.rs), [uaccess.S](../../kernel/src/arch/riscv64/uaccess.S)

### sstatus.SUM Bit

The `SUM` bit (bit 18 of sstatus) allows S-mode to access U-mode pages. Without it, any access to user-space addresses from the kernel causes a page fault.

Both Rust and assembly implementations toggle SUM around the actual copy:

```rust
fn copy_from_user(dst: *mut u8, src: *const u8, len: usize) -> usize {
    // Enable SUM
    // Perform copy
    // Disable SUM
    // Return bytes not copied (0 = success)
}
```

### Rust API

| Function | Description |
|----------|-------------|
| `access_ok(addr, size)` | Validates address is within `USER_START..USER_END` |
| `copy_to_user(dst, src, len)` | Copy kernel → user, returns bytes not copied |
| `copy_from_user(dst, src, len)` | Copy user → kernel, returns bytes not copied |
| `clear_user(addr, len)` | Zero user memory |
| `get_user<T>(ptr)` | Type-safe single value read |
| `put_user<T>(val, ptr)` | Type-safe single value write |
| `strncpy_from_user(dst, src, max)` | Copy null-terminated string |
| `strnlen_user(addr, max)` | Measure string length in user space |

### Assembly Optimized Path (`uaccess.S`)

The assembly implementation provides an optimized copy with three phases:

1. **Alignment**: Byte copy until destination is 8-byte aligned
2. **Word copy**: 8x unrolled (64 bytes/iteration), with shift-copy for misaligned source
3. **Tail**: Byte copy for remaining bytes

**Exception table mechanism**: The `EXTABLE insn, fixup` macro places entries in `.ex_table` section. If a load/store faults during the copy, the exception handler looks up the fixup address and jumps there, returning the number of uncopied bytes.

Constants:
- `WORD_COPY_MIN` = 71 bytes (threshold for word-copy path)
- `UNROLL_SIZE` = 64 bytes (8 words per loop iteration)

---

## SBI Interface

**File**: [sbi.rs](../../kernel/src/sbi.rs)

The kernel uses SBI (Supervisor Binary Interface) calls for operations that require M-mode access:

| Extension | ID (ASCII) | Functions |
|-----------|------------|-----------|
| PUT_CHAR | 0x01 | Console output (early boot) |
| SHUTDOWN | 0x08 | System shutdown |
| HART_START | 0x48534D | Start secondary CPU |
| SEND_IPI | 0x735049 | Send inter-processor interrupt |
| SET_TIMER | 0x54494D | Set timer (legacy, before SSTC) |

SBI calls use the `ecall` instruction with extension ID in a7, function ID in a6, and arguments in a0-a5.

### Timer: SSTC Extension

Since Phase 28, the kernel uses the RISC-V SSTC extension directly instead of SBI timer calls:

```rust
// Set timer compare value directly
asm!("csrw stimecmp, {0}", in(reg) deadline);
```

This avoids an SBI ecall on every timer interrupt, reducing latency.

---

## ASID and TLB Management

**File**: [asid.rs](../../kernel/src/arch/riscv64/mm/asid.rs)

### ASID Allocation

Sv39 supports 9-bit ASIDs (0-511):

| ASID | Usage |
|------|-------|
| 0 | Kernel (no ASID tagging) |
| 1 | Reserved |
| 2-511 | User processes (510 available) |

Allocation uses an atomic bitmap with CAS retry. On ASID exhaustion, the allocator linearly scans from bit 2.

### SATP Register Layout

```
Bit(s)   Field     Description
------   -----     -----------
63:60    MODE      8 = Sv39
59:44    ASID      16-bit ASID field (lower 9 bits used)
43:0     PPN       Physical page number of page table root
```

`build_satp(asid, ppn)` = `(8u64 << 60) | ((asid as u64) << 44) | ppn`

### TLB Flush Operations

| Function | Instruction | Scope |
|----------|-------------|-------|
| `flush_tlb_all()` | `sfence.vma zero, zero` | All entries |
| `flush_tlb_asid(asid)` | `sfence.vma zero, asid` | All entries for one ASID |
| `flush_tlb_page(vaddr, asid)` | `sfence.vma vaddr, asid` | Single page |
| `flush_tlb_range(start, end, asid)` | Loop `sfence.vma` | Page range |
| `flush_tlb_kernel()` | `sfence.vma zero, 0` | Kernel ASID only |

Note: `switch_mm()` does NOT embed ASID in SATP (passes ASID=0). ASID-tagged TLB flushes are used separately via `flush_tlb_asid()` when a process's address space is reassigned.

---

## SMP and IPI

**Files**: [smp.rs](../../kernel/src/arch/riscv64/smp.rs), [ipi.rs](../../kernel/src/arch/riscv64/ipi.rs)

### CPU ID Detection

S-mode cannot read `mhartid` directly. The kernel uses the tp register convention:

- **Early boot**: tp contains hart_id directly (small value < 0x1000, set by boot.S)
- **After scheduler**: tp = task_struct pointer, read `ti_cpu` at offset 0x18

### Boot Sequence

1. OpenSBI starts all harts simultaneously (HSM extension)
2. First hart to reach `init()` via CAS becomes the boot hart
3. Boot hart initializes all subsystems
4. Non-boot harts wait on `SMP_INIT_DONE` flag using `wfi`
5. Boot hart sets `SMP_INIT_DONE`, non-boot harts proceed to idle loop

### Per-CPU Interrupt Stacks

- 16KB per CPU, 16-byte aligned
- Stored in `PER_CPU_INTR_STACKS` array
- Base address exported to assembly as `__per_cpu_intr_stacks_base`
- Used for kernel-mode traps (not user traps, which use task kernel stacks)

### IPI Mechanism

| IPI Type | Value | Purpose |
|----------|-------|---------|
| Reschedule | 0 | Notify target CPU to call `schedule()` |
| Stop | 1 | Halt target CPU (infinite `wfi` loop) |

**Send**: Uses SBI IPI extension (`sbi::send_ipi(target_cpu)`)
**Receive**: Supervisor Software Interrupt (SSIP), enabled via `csrsi sie, 2`

---

## Memory Ordering

RISC-V has a weak memory model. The kernel uses the following fence instructions:

| Instruction | Equivalent | Usage |
|-------------|------------|-------|
| `fence` | DMB/DSB | Data memory barrier |
| `fence.i` | ISB | Instruction cache barrier |
| `fence iorw, iorw` | Full barrier | Before `sret` to user mode |
| `amoswap.d.aq` | Atomic with acquire | Kernel big lock acquire |
| `amoswap.d.rl` | Atomic with release | Kernel big lock release |
| `sfence.vma` | TLB barrier | After SATP change or page table update |

### Kernel Big Lock

The kernel uses a single global spinlock for SMP safety, implemented with RISC-V atomics:

```asm
# Acquire
.Lacquire:
    li    t0, 1
    amoswap.d.aq t1, t0, (lock_addr)
    bnez  t1, .Lacquire      # Spin if already locked

# Release
    amoswap.d.rl x0, x0, (lock_addr)
```

---

## RISC-V Instructions Reference

Key instructions used across the kernel:

### CSR Access

```
csrr   rd, csr         # Read CSR
csrw   csr, rs         # Write CSR
csrs   csr, rs         # Set bits in CSR
csrc   csr, rs         # Clear bits in CSR
csrrw  rd, csr, rs     # Atomic read-and-write CSR
csrsi  csr, imm        # Set immediate bits
csrci  csr, imm        # Clear immediate bits
```

### Trap and Control

```
ecall                  # Environment call (system call / SBI)
sret                   # Return from S-mode trap
wfi                    # Wait for interrupt
```

### TLB Management

```
sfence.vma             # Flush all TLB entries
sfence.vma rs1, rs2    # Flush entries matching ASID (rs2) and/or address (rs1)
```

### Atomics

```
amoswap.d.aq rd, rs2, (rs1)   # Atomic swap (acquire semantics)
amoswap.d.rl rd, rs2, (rs1)   # Atomic swap (release semantics)
sc.d rd, rs2, (rs1)           # Store-conditional (0 on success)
```

### FPU

```
fsd  fs, offset(rs)     # Store 64-bit float
fld  fs, offset(rs)     # Load 64-bit float
frcsr rd                # Read float CSR
fscsr rs                # Write float CSR
```

---

## References

### Specifications
- [RISC-V Privileged Architecture v20211203](https://riscv.org/technical/specifications/)
- [RISC-V Unprivileged ISA v20191213](https://riscv.org/technical/specifications/)
- [RISC-V Linux ABI](https://github.com/riscv-non-isa/riscv-elf-psabi-doc)

### Linux Kernel Reference
- [Linux arch/riscv/kernel/entry.S](https://elixir.bootlin.com/linux/latest/source/arch/riscv/kernel/entry.S) - Trap entry/exit
- [Linux arch/riscv/kernel/process.c](https://elixir.bootlin.com/linux/latest/source/arch/riscv/kernel/process.c) - Context switch, start_thread
- [Linux arch/riscv/mm/tlbflush.c](https://elixir.bootlin.com/linux/latest/source/arch/riscv/mm/tlbflush.c) - TLB flush
- [Linux arch/riscv/include/asm/uaccess.h](https://elixir.bootlin.com/linux/latest/source/arch/riscv/include/asm/uaccess.h) - User access

### Open Source Projects
- [OpenSBI](https://github.com/riscv/opensbi)
- [QEMU RISC-V virt Platform](https://www.qemu.org/docs/master/system/riscv/virt.html)

---

**Document Version**: v3.0
**Last Updated**: 2026-03-27
**Maintainer**: Rux Development Team
