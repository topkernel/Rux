# Linux vs Rux 启动序列对比

## 概述

本文档对比 Linux 和 Rux 内核的启动序列，重点关注：
1. tp (thread pointer) 寄存器的初始化
2. sscratch CSR 的设置时机
3. 从早期启动到调度器模式的过渡

## 1. Linux 启动序列

### 1.1 启动 CPU (Boot CPU)

**文件**: `arch/riscv/kernel/head.S`

```asm
// head.S:307 - MMU 启用前
la tp, init_task              // tp 立即指向 init_task
la sp, init_thread_union + THREAD_SIZE

// head.S:330-333 - MMU 启用后（重定位）
la tp, init_task              // 重新加载 tp
la sp, init_thread_union + THREAD_SIZE
addi sp, sp, -PT_SIZE_ON_STACK
scs_load_current

// head.S:328 - 设置 trap 向量
call .Lsetup_trap_vector
```

**`.Lsetup_trap_vector`**:
```asm
// head.S:189-199
.Lsetup_trap_vector:
    la a0, handle_exception
    csrw CSR_TVEC, a0

    // 关键：设置 sscratch = 0，表示当前在内核态
    csrw CSR_SCRATCH, zero
    ret
```

### 1.2 次级 CPU (Secondary CPUs)

**SBI HSM 方式** (`cpu_ops_sbi.c`):
```c
// 启动次级 CPU 时传递 idle task 指针
bdata->task_ptr = tidle;
bdata->stack_ptr = task_pt_regs(tidle);
sbi_hsm_hart_start(hartid, boot_addr, hsm_data);
```

**次级 CPU 入口** (`head.S:128-163`):
```asm
secondary_start_sbi:
    // 从 SBI 传递的 boot data 加载 tp 和 sp
    li a2, SBI_HART_BOOT_TASK_PTR_OFFSET
    add a2, a2, a1
    REG_L tp, (a2)              // tp = idle task 指针

    // ... MMU 设置 ...

    call .Lsetup_trap_vector    // 设置 sscratch = 0
    call smp_callin
```

### 1.3 Linux 的 tp/sscratch 协议

| 阶段 | tp 值 | sscratch 值 | 说明 |
|------|-------|-------------|------|
| 内核态运行 | `current` task_struct | 0 | sscratch=0 表示内核态 |
| 用户态运行 | 用户 TLS | `current` task_struct | sscratch 保存 task 指针 |
| Trap 入口（来自内核） | 不变 | 0 | csrrw tp, sscratch, tp 后 tp=0 |
| Trap 入口（来自用户） | 用户 TLS → task | task → 用户 TLS | csrrw 交换后 tp=task |

**Trap 入口检测** (`entry.S:96-106`):
```asm
handle_exception:
    csrrw tp, CSR_SCRATCH, tp   // 原子交换
    bnez tp, .Lsave_context     // tp != 0 表示来自用户态
                                // tp == 0 表示来自内核态
```

**返回用户态前** (`entry.S:236-239`):
```asm
    // 保存 tp 到 sscratch，以便下次 trap 时能找到内核数据结构
    csrw CSR_SCRATCH, tp
```

---

## 2. Rux 启动序列

### 2.1 启动 CPU

**文件**: `kernel/src/arch/riscv64/boot.S`

```asm
_start:
    // a0 = hart_id (OpenSBI 传递)
    mv tp, a0                    // tp = hart_id (不是 task 指针!)

    // 计算 per-CPU 栈
    li t1, 65536
    mul t1, tp, t1
    la sp, _stack_bottom
    add sp, sp, t1
    addi sp, sp, 65536

    // 清零 BSS (仅第一个 hart)
    // ...

    call rust_main               // 跳转到 Rust 代码
```

### 2.2 rust_main 初始化流程

**文件**: `kernel/src/main.rs`

```rust
fn rust_main() -> ! {
    // 1. SMP 初始化
    let is_boot_hart = arch::smp::init();

    // 2. 控制台初始化
    console::init();

    // 3. Trap 初始化 (安装 stvec)
    arch::trap::init();

    // ... MMU、堆、文件系统等初始化 ...

    // 4. 调度器初始化 (创建 idle task)
    sched::init();               // <-- 这里创建 idle task

    // 5. 启动 init 进程
    init::init();

    // 6. 进入调度循环
    sched::cpu_idle_loop();
}
```

### 2.3 调度器初始化

**文件**: `kernel/src/sched/sched.rs`

```rust
pub fn init() {
    let cpu_id = crate::arch::cpu_id() as usize;
    init_per_cpu_rq(cpu_id);

    // 创建 idle task
    let idle_ptr = IDLE_TASK_STORAGES[cpu_id].as_mut_ptr();
    Task::new_idle_at(idle_ptr);

    // 设置运行队列
    rq_inner.idle = idle_ptr;
    rq_inner.current = idle_ptr;

    // 注意：此时 tp 仍然是 hart_id，不是 idle task 指针!
}
```

### 2.4 Rux 的 tp/sscratch 状态

| 阶段 | tp 值 | sscratch 值 | 说明 |
|------|-------|-------------|------|
| 启动 (boot.S) | hart_id | 未定义 | OpenSBI 传递 |
| Rust 初始化 | hart_id | 未定义 | 未设置 |
| 调度器初始化后 | hart_id | 未定义 | **问题：tp 未更新** |
| 用户态运行 | hart_id | 未定义 | 使用 sstatus.SPP 检测 |

---

## 3. 关键差异分析

### 3.1 tp 寄存器使用

| 方面 | Linux | Rux |
|------|-------|-----|
| 启动时 | `init_task` 指针 | hart_id |
| 调度后 | `current` task 指针 | hart_id (未改变) |
| 上下文切换时 | 更新为新 task | 不更新 |

### 3.2 sscratch 使用

| 方面 | Linux | Rux |
|------|-------|-----|
| 内核态 | 0 | 未定义 |
| 用户态 | task 指针 | 未定义 |
| 检测方式 | csrrw 交换 | sstatus.SPP |

### 3.3 Trap 检测机制

**Linux (sscratch 交换)**:
```asm
// 2 条指令完成检测和 tp 保存
csrrw tp, CSR_SCRATCH, tp
bnez tp, .Lfrom_user
```

**Rux (sstatus.SPP)**:
```asm
// 3+ 条指令
csrr t0, sstatus
andi t0, t0, SR_SPP
bnez t0, .Lfrom_kernel
```

---

## 4. 实现 sscratch 检测的条件

要让 Rux 使用 Linux 风格的 sscratch 检测，需要满足：

### 4.1 tp 指向 task_struct

**当前问题**: tp = hart_id，不是有效的 task 指针

**解决方案**: 在调度器初始化后，设置 tp = idle_task

### 4.2 sscratch 协议

| 状态 | sscratch | tp |
|------|----------|-----|
| 内核态 | 0 | current task |
| 用户态 | current task | user TLS |

### 4.3 过渡期处理

**问题**: 从 tp = hart_id 到 tp = task_struct 的过渡期如何处理？

**Linux 方案**: 从第一条指令开始 tp 就是 task 指针，不存在过渡期

**Rux 方案**: 需要在某个安全点切换 tp

---

## 5. 安全实现方案

### 5.1 方案 A：早期切换（推荐）

在 `sched::init()` 中切换 tp：

```rust
pub fn init() {
    let cpu_id = crate::arch::cpu_id() as usize;
    init_per_cpu_rq(cpu_id);

    // 创建 idle task
    let idle_ptr = IDLE_TASK_STORAGES[cpu_id].as_mut_ptr();
    Task::new_idle_at(idle_ptr);

    // 设置 ti_cpu 字段
    (*idle_ptr).set_cpu(cpu_id);

    // 设置 sscratch = 0 (内核态)
    unsafe {
        core::arch::asm!("csrw sscratch, zero");
    }

    // 切换 tp 指向 idle task
    unsafe {
        core::arch::asm!("mv tp, {0}", in(reg) idle_ptr);
    }

    // 设置运行队列
    rq_inner.idle = idle_ptr;
    rq_inner.current = idle_ptr;
}
```

**优点**:
- 过渡期短，在 sched::init() 完成后立即生效
- 不需要修改 boot.S

**注意**:
- 必须在 sched::init() 后才能使用 sscratch 检测
- cpu_id() 需要同时支持两种模式

### 5.2 方案 B：boot.S 中初始化

类似 Linux，在 boot.S 中设置 tp = init_task：

```asm
_start:
    mv tp, a0                    // 暂存 hart_id

    // ... 栈设置 ...

    // 为每个 CPU 创建静态 idle task
    la t0, idle_tasks
    slli t1, tp, 3               // t1 = hart_id * 8
    add t0, t0, t1
    ld tp, (t0)                  // tp = &idle_tasks[hart_id]

    // 设置 sscratch = 0
    csrw sscratch, zero

    call rust_main
```

**优点**:
- 从一开始就与 Linux 一致
- 不存在过渡期

**缺点**:
- 需要在 boot.S 中分配 idle task（复杂）
- 需要确保 idle task 在 BSS 清零后初始化

### 5.3 推荐方案：方案 A + 兼容检测

使用方案 A，但 trap.S 需要兼容两种模式：

```asm
trap_entry:
    csrrw tp, sscratch, tp       // 尝试交换

    // 检查 sscratch 是否已初始化
    // 如果 sscratch == 0 且 tp 原来是小数值，说明还在早期启动
    li t0, 0x1000
    bltu tp, t0, .Learly_boot    // tp < 0x1000，早期启动

    bnez tp, .Lfrom_user         // 正常的 sscratch 检测
    j .Lfrom_kernel

.Learly_boot:
    // 早期启动阶段，使用 sstatus.SPP 检测
    csrr t0, sstatus
    andi t0, t0, SR_SPP
    bnez t0, .Lfrom_kernel
    j .Lfrom_user
```

---

## 6. 实现步骤

### 阶段 1：准备工作（已完成）
- [x] Task 结构体添加 thread_info 字段
- [x] ThreadStruct 添加上下文字段
- [x] context_switch 添加 SUM 位保存/恢复

### 阶段 2：调度器初始化修改
1. 在 `sched::init()` 中设置 tp = idle_task
2. 设置 sscratch = 0
3. 设置 idle task 的 ti_cpu 字段

### 阶段 3：trap.S 修改
1. 使用 sscratch 交换检测
2. 添加早期启动兼容检测
3. 返回用户态时设置 sscratch = tp

### 阶段 4：cpu_id() 更新
1. 检测 tp 模式（hart_id vs task_struct）
2. 根据模式选择不同的获取方式

### 阶段 5：测试验证
1. 验证内核启动正常
2. 验证 shell 启动正常
3. 验证用户程序运行正常

---

## 7. 风险评估

| 风险 | 影响 | 缓解措施 |
|------|------|----------|
| 早期 trap 处理 | 高 | 添加早期启动兼容检测 |
| tp 切换时机 | 中 | 在 sched::init() 末尾切换 |
| cpu_id() 兼容 | 中 | 支持两种模式检测 |
| sscratch 竞争 | 低 | 单核环境下不存在 |
