# Rux 内核代码审查报告

## 与 Linux 内核对比分析

**审查日期**: 2026-02-24
**对比版本**: Linux 6.x (refer/linux)
**审查范围**: 核心内核子系统

---

## 一、整体评估

Rux 项目整体架构合理，基本遵循了 Linux 的设计模式。但与 Linux 内核相比，存在多处需要重构和改进的地方。

**已实现的核心功能**:
- RISC-V Sv39 虚拟内存管理
- 进程调度（Round Robin）
- VFS 文件系统层 + ext4
- 基本系统调用
- SMP 多核支持

**主要差距领域**:
- 数据结构布局与 Linux 不完全兼容
- 错误处理路径不完整
- 架构抽象层缺失
- 性能优化不足

---

## 二、需要重构的关键问题

### 1. [P0] TrapFrame/pt_regs 结构体布局不一致 ✅ 已修复

**文件**: `kernel/src/arch/riscv64/pt_regs.rs` (新建)
**优先级**: 高
**状态**: ✅ **已修复** (2026-02-24)

**Linux 实现** (`arch/riscv/include/asm/ptrace.h`):
```c
struct pt_regs {
    unsigned long epc;      // PC 在最前面
    unsigned long ra;
    unsigned long sp;
    unsigned long gp;
    unsigned long tp;
    unsigned long t0-t6;    // 临时寄存器
    unsigned long s0-s11;   // 保存寄存器
    unsigned long a0-a7;    // 参数寄存器
    unsigned long status;   // CSR
    unsigned long badaddr;  // CSR (stval)
    unsigned long cause;    // CSR (scause)
    unsigned long orig_a0;  // 原始 a0（系统调用回滚需要）
};
```

**Rux 当前实现**:
```rust
pub struct TrapFrame {
    pub ra: u64,   // 从 sp+16 开始
    pub t0-t6: u64,
    pub a0-a7: u64,
    pub s2-s11: u64,
    pub gp: u64,
    pub _pad: u64,  // 多余的填充字段
    pub sstatus: u64,
    pub sepc: u64,
    pub stval: u64,
    // 缺少 cause 和 orig_a0
}
```

**问题列表**:
1. 字段顺序与 Linux 完全不一致
2. 缺少 `orig_a0` 字段（系统调用回滚需要）
3. 缺少 `cause` 字段（异常原因）
4. `sp` 保存在 TrapFrame 之外，增加复杂性
5. 无法使用 `task_pt_regs()` 宏

**重构方案**:
- ✅ 重新设计 `PtRegs` 结构体，与 Linux `pt_regs` 布局一致
- ✅ 添加 `orig_a0` 和 `cause` 字段
- ✅ 更新 `trap.S` 汇编代码以匹配新布局
- ✅ 更新 `fork.rs`, `task.rs`, `sched.rs`, `usermod.rs` 使用新结构

**修复详情**:
- 创建了 `kernel/src/arch/riscv64/pt_regs.rs`，定义与 Linux 兼容的 `PtRegs` 结构
- 添加了 `Cause` 枚举表示异常原因
- 添加了 `user_mode()`, `syscall_get_arguments()` 等辅助方法
- 更新了 `trap.S` 以使用新的寄存器布局 (288 字节)
- 统一了 `SyscallFrame` 和 `TrapFrame` 为 `PtRegs`
- 修复了用户 sp 在 trap 入口时未正确保存的 bug

---

### 2. [P0] 系统调用处理架构问题 ✅ 已修复

**文件**: `kernel/src/arch/riscv64/syscall.rs`
**优先级**: 高
**状态**: ✅ **已修复** (2026-02-24)

**问题描述**:
1. 存在两套寄存器结构 (`TrapFrame` 和 `SyscallFrame`)，增加维护复杂度
2. 每次系统调用都需要复制寄存器，效率低
3. 缺少系统调用号边界检查
4. 缺少 `array_index_nospec` 安全措施

**Linux 实现**:
```c
// 统一的系统调用接口
typedef long (*syscall_t)(const struct pt_regs *);

static inline void syscall_get_arguments(struct task_struct *task,
                     struct pt_regs *regs,
                     unsigned long *args)
{
    args[0] = regs->orig_a0;
    args[1] = regs->a1;
    // ...
}

// 系统调用表
void * const sys_call_table[__NR_syscalls] = {
    [__NR_read] = sys_read,
    [__NR_write] = sys_write,
    // ...
};
```

**重构方案**:
- ✅ 统一使用 `PtRegs` 作为系统调用参数传递
- ✅ 添加 `syscall_get_arguments` 辅助函数
- ✅ 添加 `syscall_set_return_value` 辅助函数
- ⏳ 函数指针数组形式的系统调用表（待实现）

**修复详情**:
- `syscall_handler` 现在接受 `&mut PtRegs` 参数
- 添加了 `syscall_get_nr()`, `syscall_get_arguments()`, `syscall_set_return_value()` 辅助函数
- 使用 `orig_a0` 作为第一个参数（支持系统调用回滚）

---

### 3. [P1] Task 结构体设计问题

**文件**: `kernel/src/process/task.rs`
**优先级**: 中
**状态**: 待修复

**问题列表**:
1. `AddressSpace` 直接嵌入 Task，导致结构体过大
2. 缺少 `thread_struct` 抽象（架构相关状态）
3. 进程状态使用枚举而非位图，无法表达组合状态
4. 缺少 `mm` 和 `active_mm` 的区分

**Linux 状态定义**:
```c
#define TASK_RUNNING         0x00000000
#define TASK_INTERRUPTIBLE   0x00000001
#define TASK_UNINTERRUPTIBLE 0x00000002
#define __TASK_STOPPED       0x00000004
#define __TASK_TRACED        0x00000008
#define EXIT_DEAD            0x00000010
#define EXIT_ZOMBIE          0x00000020
// 可以组合使用
```

**重构方案**:
```rust
pub struct Task {
    state: AtomicU32,               // 位图形式
    pub mm: Option<Arc<MmStruct>>,  // 引用计数指针
    pub active_mm: Option<Arc<MmStruct>>,
    pub files: Option<Arc<FilesStruct>>,
    pub thread: ThreadStruct,       // 架构相关部分
    // ...
}
```

---

### 4. [P1] 缺少 copy_thread / start_thread 抽象

**文件**: `kernel/src/arch/riscv64/` (需要新建)
**优先级**: 中
**状态**: 待实现

**Linux 实现**:
```c
// execve 启动新程序
void start_thread(struct pt_regs *regs, unsigned long pc, unsigned long sp)
{
    regs->status = SR_PIE;
    regs->epc = pc;
    regs->sp = sp;
}

// fork 复制线程状态
int copy_thread(struct task_struct *p, const struct kernel_clone_args *args)
{
    struct pt_regs *childregs = task_pt_regs(p);
    *childregs = *current_pt_regs();
    childregs->a0 = 0;  // fork 在子进程返回 0
    p->thread.ra = (unsigned long)ret_from_fork;
}
```

**重构方案**:
- 创建 `kernel/src/arch/riscv64/process.rs`
- 实现 `start_thread`, `copy_thread`, `flush_thread`

---

### 5. [P0] 页故障处理不完整

**文件**: `kernel/src/arch/riscv64/trap.rs`
**优先级**: 高
**状态**: 待修复

**当前问题**:
```rust
ExceptionCause::LoadPageFault => {
    // ...
    (*frame).sepc += 4;  // 错误：跳过指令而非重新执行或发送信号
}
```

**问题列表**:
1. 页故障后跳过指令是错误的，应该重新执行或发送信号
2. 缺少内核页故障的 `fixup_exception` 机制
3. 没有 OOM 处理
4. 缺少对 `VM_FAULT_SIGSEGV` 等返回值的正确处理
5. 缺少 `bad_area` / `no_context` 等标准处理路径

**Linux 处理流程**:
```c
void handle_page_fault(struct pt_regs *regs)
{
    // 1. 区分内核/用户模式
    // 2. 检查中断上下文
    // 3. 查找 VMA
    // 4. 验证权限
    // 5. 处理 COW
    // 6. 处理匿名页
    // 7. 处理 swap（如果有）
    // 8. 发送信号或 OOM
}
```

**重构方案**:
- 创建 `kernel/src/arch/riscv64/mm/fault.rs`
- 实现完整的 `do_page_fault` 函数
- 添加异常表机制

---

### 6. [P0] sscratch 寄存器管理 bug ✅ 已修复

**文件**: `kernel/src/arch/riscv64/trap.S`
**优先级**: 高
**状态**: ✅ **已修复** (2026-02-24)

**问题描述**:
在返回用户空间时，`sscratch` 被错误地设置为 0，导致后续 trap 无法正确识别 CPU ID。

**Bug 代码** (`.Lreturn_user`):
```assembly
// 错误代码
csrw sscratch, zero    // 设置 sscratch = 0
sret
```

**影响**:
1. 当从用户空间再次进入 trap 时，`csrrw tp, sscratch, tp` 将 `tp` 设为 0
2. `addi tp, tp, -1` 将 `tp` 设为 -1 (0xFFFFFFFFFFFFFFFF)
3. `cpu_id()` 返回无效值，导致无法找到当前任务的运行队列
4. 系统调用（如 `read`）无法找到当前进程的文件描述符表
5. Shell 在启动后立即退出

**修复方案**:
在恢复用户 `tp` 之前，先保存内核 hart ID 并设置 `sscratch = hart_id + 1`：

```assembly
.Lreturn_user:
    // 在恢复用户 tp 之前，先设置 sscratch = hart_id + 1
    addi t0, tp, 1
    csrw sscratch, t0

    // 恢复用户 tp (从 PtRegs)
    ld x4, PT_TP(sp)

    // 恢复用户 sp (从 PtRegs)
    ld x2, PT_SP(sp)

    sret
```

**修复详情**:
- 修复了 `.Lreturn_user` 中的 sscratch 设置
- 修复了 `ret_from_fork` 中的相同问题
- 添加了 `cpu_id()` 函数的正确实现（使用 `tp` 寄存器而非 M-mode 的 `mhartid` CSR）

---

### 6. [P1] 内存管理架构问题

**文件**: `kernel/src/arch/riscv64/mm.rs`, `kernel/src/mm/`
**优先级**: 中
**状态**: 待改进

**问题列表**:
1. VMA 使用线性搜索 Vec，O(n) 复杂度（Linux 使用红黑树）
2. 缺少 `mm_struct` 的完整抽象
3. 页表项类型不安全，直接使用 `u64`
4. 缺少 `p4d_t` 四级页表支持

**重构方案**:
```rust
pub struct MmStruct {
    pub pgd: PhysAddr,
    pub mmap: Arc<RwLock<RbTree<Vma>>>,
    pub start_code: VirtAddr,
    pub end_code: VirtAddr,
    pub start_data: VirtAddr,
    pub end_data: VirtAddr,
    pub start_brk: VirtAddr,
    pub brk: AtomicU64,
    pub start_stack: VirtAddr,
    pub arg_start: VirtAddr,
    pub env_start: VirtAddr,
    pub total_vm: AtomicU64,
    pub locked_vm: AtomicU64,
}

// 类型安全的页表项
pub struct Pgde(PteValue);
pub struct Pude(PteValue);
pub struct Pmde(PteValue);
pub struct Pte(PteValue);
```

---

### 7. [P2] 缺少关键的辅助宏/函数

**文件**: 多处
**优先级**: 低
**状态**: 待实现

| 宏/函数 | Linux | Rux | 说明 |
|---------|-------|-----|------|
| `user_mode(regs)` | ✅ | ✅ | 检查是否来自用户态 (PtRegs::user_mode()) |
| `task_pt_regs(task)` | ✅ | ❌ | 获取任务的 pt_regs |
| `current_pt_regs()` | ✅ | ✅ | 获取当前进程的 pt_regs |
| `in_interrupt()` | ✅ | ❌ | 检查是否在中断上下文 |
| `in_task()` | ✅ | ❌ | 检查是否在进程上下文 |
| `fixup_exception()` | ✅ | ❌ | 内核异常修复 |
| `copy_to_user()` | ✅ | 部分 | 安全的用户空间复制 |
| `copy_from_user()` | ✅ | 部分 | 安全的用户空间复制 |
| `get_user()` | ✅ | ❌ | 安全读取用户空间 |
| `put_user()` | ✅ | ❌ | 安全写入用户空间 |

---

### 8. [P2] FPU/向量扩展状态保存

**文件**: 需要新建
**优先级**: 低
**状态**: 未实现

**Linux 实现**:
```c
struct thread_struct {
    unsigned long fstate[FSTATE_SIZE];  // FPU 状态
    struct __riscv_v_ext_state vstate;  // 向量扩展状态
};

// 上下文切换时保存/恢复
void fstate_save(struct task_struct *task, struct pt_regs *regs);
void fstate_restore(struct task_struct *task, struct pt_regs *regs);
```

---

## 三、重构进度跟踪

### 第一优先级（核心功能）
- [x] 1. 统一 TrapFrame/pt_regs 结构 ✅ (2026-02-24)
- [ ] 2. 修复页故障处理
- [x] 3. 统一系统调用框架 ✅ (2026-02-24)
- [x] 4. 修复 sscratch 寄存器管理 bug ✅ (2026-02-24)

### 第二优先级（架构改进）
- [ ] 4. 重构 Task 结构体
- [ ] 5. 实现 mm_struct 完整抽象
- [ ] 6. 实现 start_thread/copy_thread

### 第三优先级（功能完善）
- [ ] 7. VMA 红黑树优化
- [ ] 8. 添加异常表机制
- [ ] 9. FPU/向量扩展支持
- [ ] 10. 完善信号处理

---

## 四、代码风格指南

### 命名规范
使用 Linux 风格的函数命名：
- `do_page_fault` 而非 `handle_mm_fault`
- `copy_thread` 而非 `fork_trap_frame`
- `sys_read` 而非 `syscall_read`

### 错误处理
使用标准 Linux 错误码：
```rust
pub type LinuxResult<T> = Result<T, LinuxError>;

pub enum LinuxError {
    EPERM = 1,
    ENOENT = 2,
    ESRCH = 3,
    EINTR = 4,
    EIO = 5,
    // ...
}
```

---

## 五、参考资料

- Linux 内核源码: `refer/linux/`
- RISC-V 特权架构规范 v20211203
- POSIX 标准: https://pubs.opengroup.org/onlinepubs/9699919799/

---

*此文档将随重构进度持续更新*
