# Rux vs Linux 上下文切换对比分析

## 概述

本文档详细对比 Rux 内核与 Linux 内核在以下方面的实现差异：
1. 用户态到内核态的切换（Trap Entry）
2. 内核态到用户态的切换（Trap Exit）
3. 内核上下文切换（Context Switch）
4. 进程/线程上下文保存与恢复

## 1. 用户态/内核态检测机制

### 1.1 Rux 实现

**文件**: `kernel/src/arch/riscv64/trap.S`

Rux 使用 `sstatus.SPP` 位来判断 trap 来源：

```asm
trap_entry:
    csrr t0, sstatus
    andi t0, t0, 0x100       # 检查 SPP 位 (bit 8)
    bnez t0, .Lfrom_kernel   # SPP=1 表示来自内核
    j .Lfrom_user            # SPP=0 表示来自用户
```

**特点**:
- 直接读取 sstatus 寄存器
- 使用 SPP (Supervisor Previous Privilege) 位判断
- 逻辑简单直观

### 1.2 Linux 实现

**文件**: `refer/linux/arch/riscv/kernel/entry.S`

Linux 使用 `sscratch` 寄存器交换技巧：

```asm
SYM_CODE_START(handle_exception)
    csrrw tp, CSR_SCRATCH, tp  # 交换 tp 和 sscratch
    bnez tp, .Lsave_context    # sscratch 非 0 = 来自用户态
                               # sscratch 为 0 = 来自内核态
```

**原理**:
- 用户态运行时：`sscratch = tp (hart_id + 1)`，tp 保存用户 TLS
- 内核态运行时：`sscratch = 0`，tp 指向 task_struct
- 进入 trap 时交换 tp 和 sscratch：
  - 来自用户态：tp 变为 hart_id+1（非 0）
  - 来自内核态：tp 变为 0

**特点**:
- 单条指令完成检测和 tp 保存
- 更高效（少一次 CSR 读取）
- Linux 标准做法

### 1.3 对比总结

| 特性 | Rux | Linux |
|------|-----|-------|
| 检测方式 | sstatus.SPP 位 | sscratch 交换 |
| 指令数 | 3+ 条 | 2 条 |
| tp 用途 | 固定为 hart_id | 用户态存 TLS，内核态指向 task |
| 效率 | 较低 | 较高 |

---

## 2. 栈管理策略

### 2.1 Rux 实现

**文件**: `kernel/src/arch/riscv64/trap.S`

Rux 使用**专用 trap 栈**：

```asm
.section .bss
.align 16
__kernel_trap_stack:
    .space 16384 * 4          # 每个 CPU 16KB

.Lfrom_user:
    # 从用户态进入，加载专用 trap 栈
    csrr t0, sscratch
    sub t0, t0, #1            # t0 = hart_id
    la t1, __kernel_trap_stack
    slli t0, t0, #14          # t0 = hart_id * 16384
    add sp, t1, t0            # sp = trap_stack + offset
```

**特点**:
- 独立的 trap 栈空间
- 每个 CPU 有自己的 trap 栈
- 不会与进程内核栈混淆

### 2.2 Linux 实现

**文件**: `refer/linux/arch/riscv/kernel/entry.S`

Linux 使用**当前进程的内核栈**：

```asm
.Lsave_context:
    # 来自内核态，已经在使用内核栈
    # 不需要切换栈

.Lskip_restore:
    # 来自用户态，task_struct 的内核栈已经就绪
    # tp 指向 task_struct，sp 已经是内核栈
```

**原理**:
- 每个进程/线程创建时分配内核栈（通常 8KB-16KB）
- thread_info 嵌入在栈底部或 task_struct 开头
- tp 寄存器始终指向当前 task_struct

**特点**:
- 无需额外栈空间
- 上下文信息与栈紧密关联
- Linux 标准做法

### 2.3 对比总结

| 特性 | Rux | Linux |
|------|-----|-------|
| 栈来源 | 专用 trap 栈 | 进程内核栈 |
| 栈大小 | 固定 16KB/CPU | 每进程分配 |
| 上下文关联 | 独立存储 | 栈 + task_struct |
| 复杂度 | 较高 | 较低 |

---

## 3. 内核上下文切换

### 3.1 Rux 实现

**文件**: `kernel/src/arch/riscv64/context.rs`

```rust
#[unsafe(naked)]
pub unsafe extern "C" fn cpu_switch_to(
    next_ctx: *mut CpuContext,  // a0
    prev_ctx: *mut CpuContext   // a1
) {
    core::arch::naked_asm!(
        // 保存 prev 的 callee-saved 寄存器
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

        // 恢复 next 的 callee-saved 寄存器
        "ld ra, 0(a0)",
        "ld sp, 8(a0)",
        // ... s0-s11
        "ret",
    );
}
```

**CpuContext 结构** (112 字节):
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

### 3.2 Linux 实现

**文件**: `refer/linux/arch/riscv/kernel/entry.S`

```asm
SYM_FUNC_START(__switch_to)
    # 保存 prev 的上下文
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

    # 保存 sstatus (包括 SUM 位)
    csrr  s0, CSR_STATUS
    REG_S s0, TASK_THREAD_SUM_RA(a3)

    # Shadow Call Stack 支持
#ifdef CONFIG_SHADOW_CALL_STACK
    addi  s0, a3, TASK_TI_SCS
    REG_S s0, TASK_TI_SCS_OFFSET(a3)
#endif

    # 恢复 next 的上下文
    REG_L ra,  TASK_THREAD_RA_RA(a4)
    REG_L sp,  TASK_THREAD_SP_RA(a4)
    # ... s0-s11

    # 恢复 sstatus
    REG_L s0, TASK_THREAD_SUM_RA(a4)
    csrs  CSR_STATUS, s0

    # 更新 tp 指向新 task
    move tp, a1

    # vmalloc 检查
#ifdef CONFIG_MMU
    REG_L s0, TASK_TI_VMACTL(a4)
    bnez s0, .Lnew_vmalloc_check
#endif

    ret
SYM_FUNC_END(__switch_to)
```

### 3.3 对比总结

| 特性 | Rux | Linux |
|------|-----|-------|
| 保存寄存器 | ra, sp, s0-s11 | ra, sp, s0-s11 + sstatus |
| SUM 位处理 | 不处理 | 保存/恢复 |
| Shadow Call Stack | 不支持 | 支持 (CONFIG) |
| vmalloc 检查 | 不支持 | 支持 |
| tp 更新 | 不更新 | move tp, a1 |
| 参数传递 | next_ctx, prev_ctx 指针 | task_struct 指针 |

---

## 4. PtRegs 结构体对比

### 4.1 Rux 实现

**文件**: `kernel/src/arch/riscv64/pt_regs.rs`

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
    pub orig_a0: u64,  // 0x118 - 原始 a0
}
// 总大小: 0x120 = 288 字节
```

### 4.2 Linux 实现

**文件**: `refer/linux/arch/riscv/include/asm/ptrace.h`

```c
struct pt_regs {
    unsigned long epc;        // 0x00
    unsigned long ra;         // 0x08
    unsigned long sp;         // 0x10
    unsigned long gp;         // 0x18
    unsigned long tp;         // 0x20
    unsigned long t0;         // 0x28
    // ... 完全相同的布局 ...
    unsigned long t6;         // 0xf8
    unsigned long status;     // 0x100
    unsigned long badaddr;    // 0x108
    unsigned long cause;      // 0x110
    unsigned long orig_a0;    // 0x118
};
// 总大小: 0x120 = 288 字节
```

### 4.3 对比总结

| 特性 | Rux | Linux |
|------|-----|-------|
| 布局 | ✅ 完全一致 | 标准 |
| 大小 | ✅ 288 字节 | 288 字节 |
| orig_a0 | ✅ 支持 | 支持 |
| 字段顺序 | ✅ 一致 | 标准 |

**结论**: PtRegs 结构体与 Linux 完全兼容。

---

## 5. thread_info / Task 结构对比

### 5.1 Rux 实现

**文件**: `kernel/src/process/task.rs`

```rust
pub struct Task {
    pid: u32,
    state: TaskState,
    context: CpuContext,      // 嵌入在 Task 内
    kernel_stack: Option<...>,
    mm: Option<Arc<MmStruct>>,
    // ... 其他字段
}
```

**特点**:
- Task 是独立的结构体
- context 嵌入在 Task 中
- tp 寄存器不指向 Task

### 5.2 Linux 实现

**文件**: `refer/linux/arch/riscv/include/asm/thread_info.h`

```c
struct thread_info {
    unsigned long flags;      // 低地址
    int preempt_count;
    unsigned long kernel_sp;
    unsigned long user_sp;
    int cpu;
};

// thread_info 嵌入在 task_struct 开头
struct task_struct {
    struct thread_info thread_info;  // offset 0
    // ... 其他字段
};
```

**特点**:
- thread_info 在 task_struct 开头（offset 0）
- tp 寄存器指向 task_struct（也指向 thread_info）
- 可以快速访问 flags、preempt_count 等

### 5.3 对比总结

| 特性 | Rux | Linux |
|------|-----|-------|
| 结构组织 | Task 独立 | thread_info 嵌入 task_struct |
| tp 用途 | hart_id | 指向当前 task_struct |
| 快速访问 | 需要查找 | 直接通过 tp |
| offset 0 | 无特殊含义 | thread_info 所在 |

---

## 6. ret_from_fork 对比

### 6.1 Rux 实现

**文件**: `kernel/src/arch/riscv64/trap.S`

```asm
.global ret_from_fork
ret_from_fork:
    # 恢复上下文
    RESTORE_ALL
    # 返回
    sret
```

**特点**:
- 单一入口点
- 不区分内核线程和用户线程

### 6.2 Linux 实现

**文件**: `refer/linux/arch/riscv/kernel/entry.S`

```asm
SYM_CODE_START(ret_from_fork_kernel_asm)
    call schedule_tail
    move a0, s0              # 传递 fn
    move a1, s1              # 传递 arg
    jalr s0                  # 调用内核线程函数
    j ret_from_fork_kernel
SYM_CODE_END(ret_from_fork_kernel_asm)

SYM_CODE_START(ret_from_fork_user_asm)
    call schedule_tail
    # 返回用户态
    j ret_from_exception
SYM_CODE_END(ret_from_fork_user_asm)
```

**特点**:
- 两个入口点：内核线程和用户线程
- 内核线程直接调用函数
- 用户线程走正常返回路径

---

## 7. 缺失功能清单

### 7.1 高优先级（影响正确性）

| 功能 | 描述 | Linux | Rux |
|------|------|-------|-----|
| SUM 位保存/恢复 | 上下文切换时保持 SUM 状态 | ✅ | ❌ |
| sscratch 检测 | 使用标准方式检测 user/kernel | ✅ | ❌ |
| tp 指向 task | 快速访问当前进程 | ✅ | ❌ |

### 7.2 中优先级（影响性能/兼容性）

| 功能 | 描述 | Linux | Rux |
|------|------|-------|-----|
| thread_info 结构 | 嵌入 task_struct 开头 | ✅ | ❌ |
| 内核线程入口 | ret_from_fork_kernel | ✅ | ❌ |
| vmalloc 检查 | 切换后检查 vmalloc 区域 | ✅ | ❌ |

### 7.3 低优先级（可选优化）

| 功能 | 描述 | Linux | Rux |
|------|------|-------|-----|
| Shadow Call Stack | 安全特性 | ✅ | ❌ |
| Vector 状态保存 | V 扩展支持 | ✅ | ❌ |
| preempt_count | 抢占计数 | ✅ | ❌ |

---

## 8. 关键差异汇总

```
┌─────────────────────────────────────────────────────────────┐
│                    上下文切换关键差异                         │
├─────────────────────────────────────────────────────────────┤
│  方面                 │  Rux 当前实现     │  Linux 标准      │
├───────────────────────┼───────────────────┼──────────────────┤
│  User/Kernel 检测     │  sstatus.SPP      │  sscratch 交换   │
│  Trap 栈              │  专用 trap 栈     │  进程内核栈      │
│  tp 寄存器            │  hart_id          │  task_struct*    │
│  Context Switch       │  仅寄存器         │  寄存器 + SUM    │
│  thread_info          │  独立 Task        │  嵌入 task       │
│  ret_from_fork        │  单一入口         │  双入口          │
└─────────────────────────────────────────────────────────────┘
```
