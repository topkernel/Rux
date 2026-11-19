# Rux vs Linux Context Switch Comparison Analysis

## Overview

This document provides a detailed comparison of the implementation differences between the Rux kernel and the Linux kernel in the following aspects:
1. User mode to kernel mode switching (Trap Entry)
2. Kernel mode to user mode switching (Trap Exit)
3. Kernel context switching (Context Switch)
4. Process/thread context save and restore

## 1. User/Kernel Mode Detection Mechanism

### 1.1 Rux Implementation

**File**: `kernel/src/arch/riscv64/trap.S`

Rux uses the `sstatus.SPP` bit to determine the trap source:

```asm
trap_entry:
    csrr t0, sstatus
    andi t0, t0, 0x100       # Check SPP bit (bit 8)
    bnez t0, .Lfrom_kernel   # SPP=1 means from kernel
    j .Lfrom_user            # SPP=0 means from user
```

**Features**:
- Directly reads the sstatus register
- Uses SPP (Supervisor Previous Privilege) bit for determination
- Simple and intuitive logic

### 1.2 Linux Implementation

**File**: `refer/linux/arch/riscv/kernel/entry.S`

Linux uses the `sscratch` register swap trick:

```asm
SYM_CODE_START(handle_exception)
    csrrw tp, CSR_SCRATCH, tp  # Swap tp and sscratch
    bnez tp, .Lsave_context    # sscratch non-zero = from user mode
                               # sscratch zero = from kernel mode
```

**Principle**:
- When running in user mode: `sscratch = tp (hart_id + 1)`, tp stores user TLS
- When running in kernel mode: `sscratch = 0`, tp points to task_struct
- On trap entry, swap tp and sscratch:
  - From user mode: tp becomes hart_id+1 (non-zero)
  - From kernel mode: tp becomes 0

**Features**:
- Single instruction completes detection and tp save
- More efficient (one fewer CSR read)
- Linux standard approach

### 1.3 Comparison Summary

| Feature | Rux | Linux |
|---------|-----|-------|
| Detection method | sstatus.SPP bit | sscratch swap |
| Instruction count | 3+ instructions | 2 instructions |
| tp usage | Fixed as hart_id | User mode stores TLS, kernel mode points to task |
| Efficiency | Lower | Higher |

---

## 2. Stack Management Strategy

### 2.1 Rux Implementation

**File**: `kernel/src/arch/riscv64/trap.S`

Rux uses a **dedicated trap stack**:

```asm
.section .bss
.align 16
__kernel_trap_stack:
    .space 16384 * 4          # 16KB per CPU

.Lfrom_user:
    # Entered from user mode, load dedicated trap stack
    csrr t0, sscratch
    sub t0, t0, #1            # t0 = hart_id
    la t1, __kernel_trap_stack
    slli t0, t0, #14          # t0 = hart_id * 16384
    add sp, t1, t0            # sp = trap_stack + offset
```

**Features**:
- Independent trap stack space
- Each CPU has its own trap stack
- Does not mix with process kernel stack

### 2.2 Linux Implementation

**File**: `refer/linux/arch/riscv/kernel/entry.S`

Linux uses the **current process's kernel stack**:

```asm
.Lsave_context:
    # From kernel mode, already using kernel stack
    # No need to switch stack

.Lskip_restore:
    # From user mode, task_struct's kernel stack is ready
    # tp points to task_struct, sp is already kernel stack
```

**Principle**:
- Each process/thread is allocated a kernel stack at creation (typically 8KB-16KB)
- thread_info is embedded at the bottom of the stack or at the beginning of task_struct
- tp register always points to the current task_struct

**Features**:
- No extra stack space needed
- Context information closely associated with the stack
- Linux standard approach

### 2.3 Comparison Summary

| Feature | Rux | Linux |
|---------|-----|-------|
| Stack source | Dedicated trap stack | Process kernel stack |
| Stack size | Fixed 16KB/CPU | Per-process allocation |
| Context association | Independent storage | Stack + task_struct |
| Complexity | Higher | Lower |

---

## 3. Kernel Context Switch

### 3.1 Rux Implementation

**File**: `kernel/src/arch/riscv64/context.rs`

```rust
#[unsafe(naked)]
pub unsafe extern "C" fn cpu_switch_to(
    next_ctx: *mut CpuContext,  // a0
    prev_ctx: *mut CpuContext   // a1
) {
    core::arch::naked_asm!(
        // Save prev's callee-saved registers
        "sd ra, 0(a1)",
        "sd sp, 8(a1)",
        "sd s0, 16(a1)",
        "sd s1, 24(a1)",
        "sd s2, 32(a1)",
        "sd s3, 40(a1)",
        "sd s4, 48(a1)",
        "sd s5, 56(a1)",
        "sd s6, 64(a1)",
        "sd s7, 72(a1)",
        "sd s8, 80(a1)",
        "sd s9, 88(a1)",
        "sd s10, 96(a1)",
        "sd s11, 104(a1)",

        // Restore next's callee-saved registers
        "ld ra, 0(a0)",
        "ld sp, 8(a0)",
        // ... s0-s11
        "ret",
    );
}
```

**CpuContext Structure** (112 bytes):
```rust
#[repr(C)]
pub struct CpuContext {
    ra: u64,    // 0x00
    sp: u64,    // 0x08
    s0: u64,    // 0x10
    s1: u64,    // 0x18
    s2: u64,    // 0x20
    s3: u64,    // 0x28
    s4: u64,    // 0x30
    s5: u64,    // 0x38
    s6: u64,    // 0x40
    s7: u64,    // 0x48
    s8: u64,    // 0x50
    s9: u64,    // 0x58
    s10: u64,   // 0x60
    s11: u64,   // 0x68
}
```

### 3.2 Linux Implementation

**File**: `refer/linux/arch/riscv/kernel/entry.S`

```asm
SYM_FUNC_START(__switch_to)
    # Save prev's context
    REG_S ra,  TASK_THREAD_RA_RA(a3)
    REG_S sp,  TASK_THREAD_SP_RA(a3)
    REG_S s0,  TASK_THREAD_S0_RA(a3)
    REG_S s1,  TASK_THREAD_S1_RA(a3)
    REG_S s2,  TASK_THREAD_S2_RA(a3)
    REG_S s3,  TASK_THREAD_S3_RA(a3)
    REG_S s4,  TASK_THREAD_S4_RA(a3)
    REG_S s5,  TASK_THREAD_S5_RA(a3)
    REG_S s6,  TASK_THREAD_S6_RA(a3)
    REG_S s7,  TASK_THREAD_S7_RA(a3)
    REG_S s8,  TASK_THREAD_S8_RA(a3)
    REG_S s9,  TASK_THREAD_S9_RA(a3)
    REG_S s10, TASK_THREAD_S10_RA(a3)
    REG_S s11, TASK_THREAD_S11_RA(a3)

    # Save sstatus (including SUM bit)
    csrr  s0, CSR_STATUS
    REG_S s0, TASK_THREAD_SUM_RA(a3)

    # Shadow Call Stack support
#ifdef CONFIG_SHADOW_CALL_STACK
    addi  s0, a3, TASK_TI_SCS
    REG_S s0, TASK_TI_SCS_OFFSET(a3)
#endif

    # Restore next's context
    REG_L ra,  TASK_THREAD_RA_RA(a4)
    REG_L sp,  TASK_THREAD_SP_RA(a4)
    # ... s0-s11

    # Restore sstatus
    REG_L s0, TASK_THREAD_SUM_RA(a4)
    csrs  CSR_STATUS, s0

    # Update tp to point to new task
    move tp, a1

    # vmalloc check
#ifdef CONFIG_MMU
    REG_L s0, TASK_TI_VMACTL(a4)
    bnez s0, .Lnew_vmalloc_check
#endif

    ret
SYM_FUNC_END(__switch_to)
```

### 3.3 Comparison Summary

| Feature | Rux | Linux |
|---------|-----|-------|
| Saved registers | ra, sp, s0-s11 | ra, sp, s0-s11 + sstatus |
| SUM bit handling | Not handled | Save/restore |
| Shadow Call Stack | Not supported | Supported (CONFIG) |
| vmalloc check | Not supported | Supported |
| tp update | Not updated | move tp, a1 |
| Parameter passing | next_ctx, prev_ctx pointers | task_struct pointers |

---

## 4. PtRegs Structure Comparison

### 4.1 Rux Implementation

**File**: `kernel/src/arch/riscv64/pt_regs.rs`

```rust
#[repr(C)]
pub struct PtRegs {
    pub epc: u64,      // 0x00 - sepc CSR
    pub ra: u64,       // 0x08 - x1
    pub sp: u64,       // 0x10 - x2
    pub gp: u64,       // 0x18 - x3
    pub tp: u64,       // 0x20 - x4
    pub t0: u64,       // 0x28 - x5
    pub t1: u64,       // 0x30 - x6
    pub t2: u64,       // 0x38 - x7
    pub s0: u64,       // 0x40 - x8
    pub s1: u64,       // 0x48 - x9
    pub a0: u64,       // 0x50 - x10
    pub a1: u64,       // 0x58 - x11
    pub a2: u64,       // 0x60 - x12
    pub a3: u64,       // 0x68 - x13
    pub a4: u64,       // 0x70 - x14
    pub a5: u64,       // 0x78 - x15
    pub a6: u64,       // 0x80 - x16
    pub a7: u64,       // 0x88 - x17
    pub s2: u64,       // 0x90 - x18
    pub s3: u64,       // 0x98 - x19
    pub s4: u64,       // 0xa0 - x20
    pub s5: u64,       // 0xa8 - x21
    pub s6: u64,       // 0xb0 - x22
    pub s7: u64,       // 0xb8 - x23
    pub s8: u64,       // 0xc0 - x24
    pub s9: u64,       // 0xc8 - x25
    pub s10: u64,      // 0xd0 - x26
    pub s11: u64,      // 0xd8 - x27
    pub t3: u64,       // 0xe0 - x28
    pub t4: u64,       // 0xe8 - x29
    pub t5: u64,       // 0xf0 - x30
    pub t6: u64,       // 0xf8 - x31
    pub status: u64,   // 0x100 - sstatus
    pub badaddr: u64,  // 0x108 - stval
    pub cause: u64,    // 0x110 - scause
    pub orig_a0: u64,  // 0x118 - original a0
}
// Total size: 0x120 = 288 bytes
```

### 4.2 Linux Implementation

**File**: `refer/linux/arch/riscv/include/asm/ptrace.h`

```c
struct pt_regs {
    unsigned long epc;        // 0x00
    unsigned long ra;         // 0x08
    unsigned long sp;         // 0x10
    unsigned long gp;         // 0x18
    unsigned long tp;         // 0x20
    unsigned long t0;         // 0x28
    // ... completely identical layout ...
    unsigned long t6;         // 0xf8
    unsigned long status;     // 0x100
    unsigned long badaddr;    // 0x108
    unsigned long cause;      // 0x110
    unsigned long orig_a0;    // 0x118
};
// Total size: 0x120 = 288 bytes
```

### 4.3 Comparison Summary

| Feature | Rux | Linux |
|---------|-----|-------|
| Layout | Fully compatible | Standard |
| Size | 288 bytes | 288 bytes |
| orig_a0 | Supported | Supported |
| Field order | Consistent | Standard |

**Conclusion**: The PtRegs structure is fully compatible with Linux.

---

## 5. thread_info / Task Structure Comparison

### 5.1 Rux Implementation

**File**: `kernel/src/process/task.rs`

```rust
pub struct Task {
    pid: u32,
    state: TaskState,
    context: CpuContext,      // Embedded in Task
    kernel_stack: Option<...>,
    mm: Option<Arc<MmStruct>>,
    // ... other fields
}
```

**Features**:
- Task is an independent structure
- context is embedded in Task
- tp register does not point to Task

### 5.2 Linux Implementation

**File**: `refer/linux/arch/riscv/include/asm/thread_info.h`

```c
struct thread_info {
    unsigned long flags;      // Low address
    int preempt_count;
    unsigned long kernel_sp;
    unsigned long user_sp;
    int cpu;
};

// thread_info is embedded at the beginning of task_struct
struct task_struct {
    struct thread_info thread_info;  // offset 0
    // ... other fields
};
```

**Features**:
- thread_info is at the beginning of task_struct (offset 0)
- tp register points to task_struct (also points to thread_info)
- Fast access to flags, preempt_count, etc.

### 5.3 Comparison Summary

| Feature | Rux | Linux |
|---------|-----|-------|
| Structure organization | Independent Task | thread_info embedded in task_struct |
| tp usage | hart_id | Points to current task_struct |
| Fast access | Requires lookup | Directly via tp |
| offset 0 | No special meaning | Where thread_info is located |

---

## 6. ret_from_fork Comparison

### 6.1 Rux Implementation

**File**: `kernel/src/arch/riscv64/trap.S`

```asm
.global ret_from_fork
ret_from_fork:
    # Restore context
    RESTORE_ALL
    # Return
    sret
```

**Features**:
- Single entry point
- Does not distinguish between kernel threads and user threads

### 6.2 Linux Implementation

**File**: `refer/linux/arch/riscv/kernel/entry.S`

```asm
SYM_CODE_START(ret_from_fork_kernel_asm)
    call schedule_tail
    move a0, s0              # Pass fn
    move a1, s1              # Pass arg
    jalr s0                  # Call kernel thread function
    j ret_from_fork_kernel
SYM_CODE_END(ret_from_fork_kernel_asm)

SYM_CODE_START(ret_from_fork_user_asm)
    call schedule_tail
    # Return to user mode
    j ret_from_exception
SYM_CODE_END(ret_from_fork_user_asm)
```

**Features**:
- Two entry points: kernel threads and user threads
- Kernel threads directly call functions
- User threads follow normal return path

---

## 7. Missing Features List

### 7.1 High Priority (Affects Correctness)

| Feature | Description | Linux | Rux |
|---------|-------------|-------|-----|
| SUM bit save/restore | Maintain SUM state during context switch | Yes | No |
| sscratch detection | Use standard method to detect user/kernel | Yes | No |
| tp points to task | Fast access to current process | Yes | No |

### 7.2 Medium Priority (Affects Performance/Compatibility)

| Feature | Description | Linux | Rux |
|---------|-------------|-------|-----|
| thread_info structure | Embedded at task_struct beginning | Yes | No |
| Kernel thread entry | ret_from_fork_kernel | Yes | No |
| vmalloc check | Check vmalloc area after switch | Yes | No |

### 7.3 Low Priority (Optional Optimization)

| Feature | Description | Linux | Rux |
|---------|-------------|-------|-----|
| Shadow Call Stack | Security feature | Yes | No |
| Vector state save | V extension support | Yes | No |
| preempt_count | Preemption count | Yes | No |

---

## 8. Key Differences Summary

```
+-------------------------------------------------------------+
|                Context Switch Key Differences               |
+-------------------------------------------------------------+
|  Aspect               |  Rux Current    |  Linux Standard   |
+-----------------------+-----------------+-------------------+
|  User/Kernel Detection|  sstatus.SPP    |  sscratch swap    |
|  Trap Stack           |  Dedicated trap |  Process kernel   |
|  tp Register          |  hart_id        |  task_struct*     |
|  Context Switch       |  Registers only |  Registers + SUM  |
|  thread_info          |  Independent    |  Embedded in task |
|  ret_from_fork        |  Single entry   |  Dual entry       |
+-------------------------------------------------------------+
```
