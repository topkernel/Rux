# 上下文切换对齐计划

## 目标

将 Rux 内核的上下文切换机制与 Linux 内核对齐，确保：
1. 完全兼容 Linux ABI
2. 支持所有必要的上下文状态
3. 代码结构与 Linux 一致

## 已完成的改进

### 1. thread_info 风格字段 ✅

在 Task 结构开头添加了 thread_info 风格字段：

```rust
#[repr(C)]
pub struct Task {
    // thread_info 字段 (offset 0)
    ti_flags: AtomicU32,           // 进程标志
    ti_preempt_count: AtomicI32,   // 抢占计数
    ti_kernel_sp: AtomicU64,       // 内核栈指针
    ti_user_sp: AtomicU64,         // 用户栈指针
    ti_cpu: AtomicI32,             // 运行 CPU
    // ... 其他字段
}
```

**新增常量**:
```rust
pub const TIF_SIGPENDING: u32 = 0;
pub const TIF_NEED_RESCHED: u32 = 1;
pub const TIF_NOTIFY_RESUME: u32 = 2;
pub const TIF_UPROBE: u32 = 3;
pub const TIF_MEMDIE: u32 = 4;
```

### 2. ThreadStruct 扩展 ✅

添加了上下文切换所需的字段：

```rust
pub struct ThreadStruct {
    // 上下文切换字段
    pub ra: u64,      // 返回地址
    pub sp: u64,      // 栈指针
    pub s: [u64; 12], // s0-s11
    pub sum: u64,     // SUM 位
    // ... 其他字段
}
```

### 3. SUM 位保存/恢复 ✅

在 context_switch 中添加了 SUM 位的保存和恢复：

```rust
pub unsafe fn context_switch(prev: &mut Task, next: &mut Task) {
    // 保存当前 SUM 位状态
    let sum_status: u64;
    core::arch::asm!(
        "csrr {0}, sstatus",
        "and {0}, {0}, {1}",
        out(reg) sum_status,
        in(reg) 0x40000u64,
        options(nomem, nostack)
    );

    // 调用上下文切换
    cpu_switch_to(next_ctx, prev_ctx);

    // 更新 tp 指向新任务
    // ...

    // 恢复 SUM 位状态
    if sum_status != 0 {
        core::arch::asm!(
            "csrs sstatus, {0}",
            in(reg) 0x40000u64,
            options(nomem, nostack)
        );
    }
}
```

### 4. cpu_id() 更新 ✅

更新了 cpu_id() 函数以支持新的 tp 用法：

```rust
pub fn cpu_id() -> usize {
    unsafe {
        let tp_value: u64;
        asm!("mv {}, tp", out(reg) tp_value, options(nomem, nostack, pure));

        // 检查 tp 是否为小数值（早期启动阶段的 hart_id）
        if tp_value < 0x1000 {
            tp_value as usize
        } else {
            // tp 指向 task_struct，从 ti_cpu 字段获取 hart_id
            let cpu_ptr = (tp_value as usize + 0x18) as *const AtomicI32;
            (*cpu_ptr).load(Ordering::Relaxed) as usize
        }
    }
}
```

---

## 下一步：sscratch 检测机制实现 ✅ 已完成

### 背景分析

参考 `docs/architecture/boot-sequence-comparison.md`，Linux 和 Rux 的关键差异：

| 方面 | Linux | Rux (当前) |
|------|-------|------------|
| 启动时 tp | `init_task` 指针 | `hart_id` |
| 内核态 sscratch | 0 | 未定义 |
| 检测方式 | csrrw 交换 | sstatus.SPP |

### 实现方案

采用 **方案 A + 兼容检测**：在调度器初始化后切换 tp，trap.S 同时支持两种模式。

### 阶段 2：调度器初始化修改 ✅

**修改文件**: `kernel/src/sched/sched.rs`

在 `init()` 函数末尾添加：
1. 设置 idle task 的 ti_cpu 字段
2. 设置 sscratch = 0 (表示内核态)
3. 切换 tp 指向 idle task

### 阶段 3：trap.S 修改 ✅

**修改文件**: `kernel/src/arch/riscv64/trap.S`

1. 使用 `csrrw tp, sscratch, tp` 交换检测
2. 检测逻辑：
   - tp == 0: 来自内核态
   - tp >= 0x80000000: 来自用户态（有效的 task 指针）
   - tp < 0x80000000: 早期启动阶段（使用 sstatus.SPP）
3. 返回用户态时设置 sscratch = tp (current task)

### 阶段 4：上下文切换更新 tp ✅

**修改文件**: `kernel/src/arch/riscv64/context.rs`

1. 添加 `context_switch_asm` 纯汇编函数，在上下文切换后更新 tp
2. 更新 `switch_to_user` 在 sret 前设置 sscratch = tp

### 阶段 5：测试验证 ✅

```bash
make run
# 预期: shell 正常启动
```

---

## 验证方法

### 功能测试
```bash
make run
# 预期: shell 正常启动并可以执行命令
```

### 单元测试
```bash
make test
```

## 修改的文件

| 文件 | 修改内容 |
|------|----------|
| `kernel/src/process/task.rs` | 添加 thread_info 字段和访问器方法 ✅ |
| `kernel/src/arch/riscv64/thread.rs` | 添加上下文切换字段 ✅ |
| `kernel/src/arch/riscv64/context.rs` | 添加 SUM 位保存/恢复、tp 更新、context_switch_asm ✅ |
| `kernel/src/arch/riscv64/smp.rs` | 更新 cpu_id() ✅ |
| `kernel/src/arch/riscv64/mod.rs` | 更新 cpu_id() ✅ |
| `kernel/src/sched/sched.rs` | 添加 tp 切换和 sscratch 初始化 ✅ |
| `kernel/src/arch/riscv64/trap.S` | 添加 sscratch 检测机制 ✅ |

## 成功标准

1. ✅ 内核编译成功
2. ✅ 所有模块加载正常
3. ✅ Shell 正常启动
4. ✅ 用户程序正常运行
5. ✅ sscratch 检测机制工作正常
6. ✅ 与 Linux 内核行为一致

---

## 详细设计文档

- [上下文切换对比分析](context-switch-analysis.md)
- [启动序列对比](boot-sequence-comparison.md)
