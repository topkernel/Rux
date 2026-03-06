# 内核大锁 (Kernel Big Lock)

## 概述

Rux 内核当前使用内核大锁（Kernel Big Lock，简称 BKL）作为主要的同步机制。这是一个粗粒度的锁，确保在任何时刻只有一个 CPU 能够执行内核代码。

## 设计目标

1. **简单性**：在内核开发初期，使用单一锁简化并发控制
2. **正确性优先**：避免细粒度锁带来的死锁和数据竞争问题
3. **渐进式优化**：为后续的锁拆分预留清晰的路径

## 当前实现

### 架构图

```
┌─────────────────────────────────────────────────────────────────┐
│                        用户态进程                                │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐  ┌──────────┐        │
│  │ Process A│  │ Process B│  │ Process C│  │ Process D│        │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘  └────┬─────┘        │
└───────┼─────────────┼─────────────┼─────────────┼───────────────┘
        │ syscall/    │ page fault  │ interrupt   │ syscall
        │ trap        │             │             │
        ▼             ▼             ▼             ▼
┌─────────────────────────────────────────────────────────────────┐
│                    KERNEL_LOCK (自旋锁)                          │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │  ACQUIRE ────────────────────────────────────── RELEASE │   │
│  │    │                                               │     │   │
│  │    ▼                                               ▼     │   │
│  │  ┌─────────────────────────────────────────────────┐   │   │
│  │  │              内核临界区                          │   │   │
│  │  │  - 系统调用处理                                  │   │   │
│  │  │  - 页错误处理                                    │   │   │
│  │  │  - 中断处理                                      │   │   │
│  │  │  - 调度器操作                                    │   │   │
│  │  └─────────────────────────────────────────────────┘   │   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

### 锁的生命周期

```
用户态执行
    │
    ▼
┌─────────────┐
│ trap_entry  │ ─── KERNEL_LOCK_ACQUIRE ───▶ 锁被获取
└─────────────┘
    │
    ▼
┌─────────────┐
│ trap_handler│ ─── 处理系统调用/异常/中断
└─────────────┘
    │
    ▼
┌─────────────┐
│ trap_exit   │
└─────────────┘
    │
    ▼
┌─────────────┐
│return_user  │ ─── KERNEL_LOCK_RELEASE ───▶ 锁被释放
└─────────────┘
    │
    ▼
用户态执行
```

### 代码实现

#### 1. 锁变量定义 (kernel/src/sync/kernel_lock.rs)

```rust
/// 全局内核大锁（简单自旋锁）
#[no_mangle]
pub static mut KERNEL_LOCK: AtomicBool = AtomicBool::new(false);

/// 检查当前是否持有内核大锁
#[inline]
pub fn is_locked() -> bool {
    unsafe { KERNEL_LOCK.load(Ordering::Acquire) }
}
```

#### 2. 汇编宏 (kernel/src/arch/riscv64/trap.S)

```asm
// KERNEL_LOCK_ACQUIRE - 获取内核大锁
// 使用 amoswap.w.aq 指令，带 acquire 语义
.macro KERNEL_LOCK_ACQUIRE
    la t0, KERNEL_LOCK
    li t2, 1
1:
    amoswap.w.aq t1, t2, (t0)    // 原子交换
    bnez t1, 1b                   // 自旋等待
.endm

// KERNEL_LOCK_RELEASE - 释放内核大锁
// 使用 amoswap.w.rl 指令，带 release 语义
.macro KERNEL_LOCK_RELEASE
    la t0, KERNEL_LOCK
    amoswap.w.rl zero, zero, (t0)  // 原子写入0
.endm
```

#### 3. 使用位置

| 位置 | 操作 | 说明 |
|------|------|------|
| trap_entry (用户态→内核) | ACQUIRE | 进入内核时获取锁 |
| trap_exit → .Lreturn_user | RELEASE | 返回用户态时释放锁 |
| ret_from_fork → .Lret_from_fork_user | ACQUIRE + RELEASE | fork 子进程首次调度时 |
| handle_timer_interrupt | is_locked() | 持锁时跳过调度 |
| handle_page_fault | RELEASE + schedule | 异常终止进程时先释放锁 |
| Task::sleep | RELEASE + schedule + ACQUIRE | 睡眠前释放，唤醒后重新获取 |
| do_exit | RELEASE + schedule | 进程退出时释放锁 |

### 睡眠/阻塞处理

当系统调用需要睡眠（等待 I/O、futex、信号量等）时，必须：
1. **释放内核大锁**：让其他进程可以执行
2. **调用 schedule()**：切换到其他进程
3. **唤醒后重新获取锁**：继续执行系统调用

```rust
// 睡眠函数模板
pub fn sleep(state: TaskState) {
    // 设置睡眠状态
    current.set_state(state);

    // 释放内核大锁
    crate::sync::kernel_lock_release();

    // 调度让出 CPU
    crate::sched::schedule();

    // 唤醒后重新获取内核大锁
    crate::sync::kernel_lock_acquire();
}
```

已处理的睡眠点：
- `Task::sleep()` - 通用睡眠函数
- `wait_event!` / `wait_event_interruptible!` - 等待队列宏
- `ConditionVariable::wait()` - 条件变量
- `Semaphore::down()` - 信号量 P 操作
- `futex_wait()` - Futex 等待
- `pipe_file_read()` / `pipe_file_write()` - 管道读写
- `do_exit()` - 进程退出
- `yield_cpu()` - 主动让出 CPU
- `handle_pending_signals()` - 信号处理中的 STOP 状态

### 内核入口/出口路径分析

#### 入口路径（用户态 → 内核态）

| 路径 | 锁操作 | 说明 |
|------|--------|------|
| trap_entry → .Lfrom_user | ACQUIRE | 正常系统调用/异常/中断 |
| trap_entry → .Lfrom_kernel | 无 | 内核态中断，不需要锁 |
| trap_entry → .Learly_boot | 无 | 早期启动阶段 |
| ret_from_fork → .Lret_from_fork_user | ACQUIRE | fork 子进程首次调度 |
| switch_to_user (context_switch) | 无 | init 进程，从内核态启动 |

#### 出口路径（内核态 → 用户态）

| 路径 | 锁操作 | 说明 |
|------|--------|------|
| trap_exit → .Lreturn_user | RELEASE | 正常返回用户态 |
| trap_exit → .Lreturn_kernel | 无 | 返回内核态 |
| ret_from_fork → .Lret_from_fork_user | RELEASE | fork 子进程返回用户态 |
| switch_to_user (context_switch) | 无 | init 进程首次进入用户态 |

#### 特殊路径

1. **init 进程启动**：
   - 从 `init.rs` 创建，`ctx.sp = 0`
   - 通过 `context_switch` → `switch_to_user` 启动
   - 不经过 trap 入口，所以没有获取锁
   - 首次 trap 时会正常获取锁

2. **fork 子进程**：
   - 从 `ret_from_fork` 开始执行
   - 先获取锁（模拟 trap 入口），再释放锁（返回用户态）
   - 确保锁状态与正常 trap 一致

3. **进程睡眠/唤醒**：
   - 睡眠前释放锁，唤醒后重新获取
   - 确保睡眠期间其他进程可以获取锁

### 关键设计决策

#### 1. 为什么使用汇编宏而不是 Rust 函数？

最初使用 Rust 函数实现，但发现会导致用户态程序执行异常。原因是函数调用会影响寄存器状态（可能是由于编译器优化或调用约定问题）。

**解决方案**：直接在 trap.S 中使用内联汇编实现锁操作，避免函数调用开销和潜在的寄存器状态问题。

#### 2. 内存顺序

- **ACQUIRE**: 使用 `.aq` 修饰符，确保锁获取后的内存操作不会被重排到锁获取之前
- **RELEASE**: 使用 `.rl` 修饰符，确保锁释放前的内存操作不会被重排到锁释放之后

#### 3. 调度保护

当持有内核大锁时，定时器中断处理不会触发调度：

```rust
fn handle_timer_interrupt(regs: &mut PtRegs) {
    let is_locked = crate::sync::is_locked();
    // ... 处理定时器 ...
    if crate::sched::need_resched() && !is_locked {
        crate::sched::schedule();  // 只有不持锁时才调度
    }
}
```

## 性能影响

### 当前限制

1. **单核等效**：即使多核系统，也只有一个核能执行内核代码
2. **长临界区**：整个系统调用期间持有锁，阻塞其他 CPU
3. **中断延迟**：其他 CPU 的中断需要等待锁释放

### 适用场景

- 内核开发初期
- 单核或双核系统
- 功能验证阶段

## 拆锁规划

### 阶段 1：中断上下文分离

**目标**：允许中断处理程序并行执行

**方案**：
```
当前：
  KERNEL_LOCK ──────────────────────────────────────
              │    syscall    │ interrupt │ syscall │
              └───────────────┴───────────┴─────────┘

拆分后：
  KERNEL_LOCK ────────────────┬───────┬─────────────
              │    syscall    │       │   syscall   │
              └───────────────┘       └─────────────┘
  IRQ_LOCK    ────────────────────────┬─────────────
                              │ intr  │    intr     │
                              └───────┴─────────────┘
```

**需要**：
- 引入 `IRQ_LOCK` 保护中断处理
- 中断处理程序不访问共享数据或使用独立的锁

### 阶段 2：子系统独立锁

**目标**：不同子系统可以并行执行

**方案**：
```
┌─────────────────────────────────────────────────────────────┐
│                      锁层次结构                              │
├─────────────────────────────────────────────────────────────┤
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐          │
│  │ SCHED_LOCK  │  │  MM_LOCK    │  │  FS_LOCK    │  ...     │
│  │ 调度器      │  │ 内存管理    │  │ 文件系统    │          │
│  └─────────────┘  └─────────────┘  └─────────────┘          │
├─────────────────────────────────────────────────────────────┤
│  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐          │
│  │ PIPE_LOCK   │  │ SOCKET_LOCK │  │ SIGNAL_LOCK │  ...     │
│  │ 管道        │  │ 网络        │  │ 信号        │          │
│  └─────────────┘  └─────────────┘  └─────────────┘          │
└─────────────────────────────────────────────────────────────┘
```

**锁粒度规划**：

| 子系统 | 锁名称 | 保护内容 |
|--------|--------|----------|
| 调度器 | `sched_lock` | 运行队列、进程状态 |
| 内存管理 | `mm_lock` | 页表、VMA、地址空间 |
| 文件系统 | `fs_lock` | inode、dentry、超级块 |
| 块设备 | `bio_lock` | 缓冲区缓存 |
| 网络 | `net_lock` | socket、协议栈 |
| 信号 | `signal_lock` | 信号处理、待处理信号 |

### 阶段 3：细粒度锁

**目标**：每个数据结构有自己的锁

**方案**：

```rust
// 进程级锁
struct Task {
    lock: SpinLock,      // 保护单个进程的字段
    // ...
}

// inode 级锁
struct Inode {
    lock: RwLock,        // 读写锁，允许多读单写
    // ...
}

// Per-CPU 数据（无需锁）
struct PerCpu<T> {
    data: [T; MAX_CPUS],  // 每个 CPU 独立访问
}
```

### 阶段 4：RCU (Read-Copy-Update)

**目标**：读操作无锁

**适用场景**：
- 路径查找（dentry 缓存）
- 进程列表遍历
- 网络路由表

**方案**：
```
写操作：
  1. 复制数据
  2. 修改副本
  3. 原子替换指针
  4. 等待宽限期后释放旧数据

读操作：
  1. 直接读取（无需锁）
  2. 使用 rcu_read_lock/unlock 标记临界区
```

## 拆锁实施指南

### 步骤 1：识别共享数据

```bash
# 查找全局静态变量
grep -rn "static" kernel/src --include="*.rs" | grep -v "static fn"

# 查找可能的竞争条件
grep -rn "unsafe" kernel/src --include="*.rs"
```

### 步骤 2：确定锁层次

1. **绘制依赖图**：识别哪些子系统相互依赖
2. **定义锁顺序**：避免死锁，始终按相同顺序获取锁
3. **文档化**：每个锁的用途和获取顺序

### 步骤 3：逐步替换

```rust
// 1. 添加新锁
static SCHED_LOCK: SpinLock = SpinLock::new();

// 2. 在持锁代码中使用新锁
fn schedule() {
    let _guard = SCHED_LOCK.lock();
    // 原有代码...
}

// 3. 移除内核大锁（在确认安全后）
// KERNEL_LOCK 不再保护调度器
```

### 死锁预防

**锁顺序规则**（从外到内）：
1. `KERNEL_LOCK`（最终将移除）
2. `SCHED_LOCK`
3. `MM_LOCK`
4. `FS_LOCK`
5. `INODE_LOCK`
6. `PAGE_LOCK`

**禁止**：
- 反向获取锁
- 在持锁时调用可能获取其他锁的函数（除非明确允许）

## 测试策略

### 并发测试

```rust
#[test]
fn test_concurrent_syscalls() {
    // 多线程并发执行系统调用
    // 验证数据一致性
}

#[test]
fn test_lock_contention() {
    // 测量锁竞争情况
    // 确保无死锁
}
```

### 性能基准

```rust
#[bench]
fn bench_syscall_with_bkl(b: &mut Bencher) {
    // 测量有内核大锁时的系统调用延迟
}

#[bench]
fn bench_syscall_without_bkl(b: &mut Bencher) {
    // 测量拆锁后的系统调用延迟
}
```

## 参考

- Linux 内核锁机制：`Documentation/locking/`
- RCU 设计：`Documentation/RCU/`
- Spinlock 实现：`arch/riscv/include/asm/spinlock.h`

## 变更历史

| 日期 | 版本 | 说明 |
|------|------|------|
| 2026-03-06 | 1.0 | 初始版本，实现内核大锁 |
| 2026-03-06 | 1.1 | 完善睡眠/阻塞路径的锁处理，修复 do_exit、yield_cpu 等函数 |
