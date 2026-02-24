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

### 3. [P1] Task 结构体设计问题 ✅ 已修复

**文件**: `kernel/src/process/task.rs`, `kernel/src/arch/riscv64/thread.rs`
**优先级**: 中
**状态**: ✅ **已修复** (2026-02-24)

**问题列表** (已解决):
1. ✅ `AddressSpace` 直接嵌入 Task，导致结构体过大 → 改为 `Box<AddressSpace>`
2. ✅ 缺少 `thread_struct` 抽象 → 创建 `ThreadStruct` (thread.rs)
3. ✅ 进程状态使用枚举而非位图 → 改为 `TaskState(u32)` 位图形式
4. ✅ 缺少 `mm` 和 `active_mm` 的区分 → 添加 `active_mm` 字段

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

**修复详情**:
- 创建 `kernel/src/arch/riscv64/thread.rs` 实现 `ThreadStruct`
  - FPU 状态保存/恢复 (f0-f31 + fcsr)
  - TLS 指针支持 (tp_value)
  - fpu_init() 初始化函数
- 将 `TaskState` 改为位图形式 `TaskState(u32)`
  - 添加 `is_running()`, `is_sleeping()`, `is_dead()` 方法
  - 支持 Linux 风格的状态组合
- 将 `AddressSpace` 改为 `Box<AddressSpace>` 减小 Task 大小
- 添加 `active_mm` 字段支持内核线程借用地址空间

---

### 4. [P1] 缺少 copy_thread / start_thread 抽象 ✅ 已修复

**文件**: `kernel/src/arch/riscv64/process.rs` (新建)
**优先级**: 中
**状态**: ✅ **已修复** (2026-02-24)

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

**修复详情**:
- 创建了 `kernel/src/arch/riscv64/process.rs`
- 实现了 `start_thread(regs, pc, sp)` - 设置用户程序初始状态
- 实现了 `copy_thread(child, parent_regs)` - fork 时复制线程状态
- 实现了 `flush_thread()` - 线程状态清理（预留）
- 添加了辅助函数: `current_pt_regs()`, `task_pt_regs()`, `user_stack_pointer()`, `instruction_pointer()`, `is_user_address()`
- 添加了 `copy_from_user()` 和 `copy_to_user()` 框架（待完善异常表）

---

### 5. [P0] 页故障处理不完整 ✅ 已修复

**文件**: `kernel/src/arch/riscv64/mm/fault.rs` (新建)
**优先级**: 高
**状态**: ✅ **已修复** (2026-02-24)

**当前问题** (已修复):
```rust
ExceptionCause::LoadPageFault => {
    // ...
    (*frame).sepc += 4;  // 错误：跳过指令而非重新执行或发送信号
}
```

**问题列表** (已解决):
1. ✅ 页故障后跳过指令是错误的，应该重新执行或发送信号
2. ⏳ 缺少内核页故障的 `fixup_exception` 机制（框架已实现，待完善）
3. ✅ 没有 OOM 处理
4. ✅ 缺少对 `VM_FAULT_SIGSEGV` 等返回值的正确处理
5. ✅ 缺少 `bad_area` / `no_context` 等标准处理路径

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

**修复详情**:
- 创建了 `kernel/src/arch/riscv64/mm/fault.rs`
- 实现了 `do_page_fault(regs, access_type)` 函数
- 添加了 `bad_area()` 和 `no_context()` 标准处理路径
- 实现了 `fixup_exception()` 框架（需要链接器脚本支持后完善）
- 添加了 `send_signal()` 信号发送框架
- 定义了 `MmFaultResult` 枚举表示处理结果
- 更新了 `trap.rs` 使用新的 `do_page_fault` 函数

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

### 6. [P1] 内存管理架构问题 ✅ 已修复

**文件**: `kernel/src/mm/mm_struct.rs`, `kernel/src/arch/riscv64/mm/base.rs`
**优先级**: 中
**状态**: ✅ **已修复** (2026-02-24)

**问题列表** (已解决):
1. ✅ VMA 使用线性搜索 Vec，O(n) 复杂度 → 改用 BTreeMap + max_end 快速路径
2. ✅ 缺少 `mm_struct` 的完整抽象 → 创建 `kernel/src/mm/mm_struct.rs`
3. ⏳ 页表项类型不安全，直接使用 `u64`（待改进）
4. ⏳ 缺少 `p4d_t` 四级页表支持（待改进）

**修复详情**:
- 创建了 `kernel/src/mm/mm_struct.rs`，实现与 Linux 兼容的 `MmStruct` 结构
- 添加了完整的段范围字段：`start_code`, `end_code`, `start_data`, `end_data`
- 添加了堆管理字段：`start_brk`, `brk`
- 添加了栈管理字段：`start_stack`
- 添加了参数/环境变量字段：`arg_start`, `arg_end`, `env_start`, `env_end`
- 添加了虚拟内存统计字段：`total_vm`, `locked_vm`, `pinned_vm`, `data_vm`, `exec_vm`, `stack_vm`
- 添加了 mmap 区域字段：`mmap_base`, `mmap_legacy_base`, `highest_vm_end`
- 添加了 ELF 加载辅助方法：`setup_segment_layout()`, `setup_stack()`, `setup_argv()`, `setup_envp()`
- 更新了 `kernel/src/arch/riscv64/mm/base.rs`，将架构特定方法作为 `MmStruct` 的扩展

**MmStruct 结构**:
```rust
pub struct MmStruct {
    // 页表管理
    pub pgd: u64,                                    // 页表根 PPN
    vma_manager: RwLock<VmaManager>,                 // VMA 管理器
    space_type: PageTableType,                       // 地址空间类型

    // 段范围 (Linux 兼容)
    start_code: AtomicUsize,                         // 代码段起始
    end_code: AtomicUsize,                           // 代码段结束
    start_data: AtomicUsize,                         // 数据段起始
    end_data: AtomicUsize,                           // 数据段结束

    // 堆管理
    start_brk: AtomicUsize,                          // 堆起始地址
    brk: AtomicUsize,                                // 当前堆指针

    // 栈管理
    start_stack: AtomicUsize,                        // 栈起始地址

    // 参数和环境变量
    arg_start: AtomicUsize,                          // 参数起始
    arg_end: AtomicUsize,                            // 参数结束
    env_start: AtomicUsize,                          // 环境变量起始
    env_end: AtomicUsize,                            // 环境变量结束

    // 虚拟内存统计
    total_vm: AtomicU64,                             // 总虚拟内存页数
    locked_vm: AtomicU64,                            // 锁定的内存页数
    // ... 更多字段
}
```

---

### 7. [P2] 缺少关键的辅助宏/函数

**文件**: 多处
**优先级**: 低
**状态**: 部分实现

| 宏/函数 | Linux | Rux | 说明 |
|---------|-------|-----|------|
| `user_mode(regs)` | ✅ | ✅ | 检查是否来自用户态 (PtRegs::user_mode()) |
| `task_pt_regs(task)` | ✅ | ✅ | 获取任务的 pt_regs (process.rs) |
| `current_pt_regs()` | ✅ | ✅ | 获取当前进程的 pt_regs |
| `in_interrupt()` | ✅ | ⏳ | 检查是否在中断上下文（框架已实现） |
| `in_task()` | ✅ | ❌ | 检查是否在进程上下文 |
| `fixup_exception()` | ✅ | ⏳ | 内核异常修复（框架已实现） |
| `copy_to_user()` | ✅ | ⏳ | 安全的用户空间复制（框架已实现） |
| `copy_from_user()` | ✅ | ⏳ | 安全的用户空间复制（框架已实现） |
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
- [x] 2. 修复页故障处理 ✅ (2026-02-24)
- [x] 3. 统一系统调用框架 ✅ (2026-02-24)
- [x] 4. 修复 sscratch 寄存器管理 bug ✅ (2026-02-24)

### 第二优先级（架构改进）
- [x] 5. 重构 Task 结构体 ✅ (2026-02-24)
- [x] 6. 实现 mm_struct 完整抽象 ✅ (2026-02-24)
- [x] 7. 实现 start_thread/copy_thread ✅ (2026-02-24)

### 第三优先级（功能完善）
- [x] 8. VMA 红黑树优化 ✅ (2026-02-24) - BTreeMap + max_end 快速路径
- [ ] 9. 完善异常表机制（框架已实现）
- [ ] 10. FPU/向量扩展支持（ThreadStruct 已创建，待集成上下文切换）
- [ ] 11. 完善信号处理

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
