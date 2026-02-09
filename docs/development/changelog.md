# Rux OS 变更日志

本文档记录 Rux 内核的重要变更和修复。

## [Unreleased]

### 2025-02-09

#### ✨ 新增

**RISC-V 系统调用完整实现** (Phase 10 完成)
- 实现完整的系统调用处理框架
- 用户程序可以成功调用系统调用并正常退出
- 修复 sscratch 寄存器管理，支持连续系统调用

**核心功能**：
1. **Trap 处理机制** (`kernel/src/arch/riscv64/trap.S`, `trap.rs`)
   - 使用 sscratch 寄存器在用户栈和内核栈之间快速切换
   - 在内核栈上保存 272 字节的 TrapFrame
   - 正确处理系统调用、异常和中断

2. **系统调用分发器** (`kernel/src/arch/riscv64/syscall.rs`)
   - 遵循 RISC-V Linux ABI 约定
   - 系统调用号通过 a7 寄存器传递
   - 参数通过 a0-a5 寄存器传递
   - 返回值通过 a0 寄存器返回

3. **用户模式切换** (`kernel/src/arch/riscv64/usermode_asm.S`)
   - 使用 sret 指令从特权模式切换到用户模式
   - Linux 风格单一页表方法（不切换 satp）
   - 正确设置 sstatus.SPP=0 确保返回用户模式

4. **用户程序支持** (`userspace/hello_world/`)
   - 实现 no_std 用户程序
   - 内联汇编系统调用包装函数
   - 自定义链接器脚本 (user.ld) 链接到用户空间地址

**技术细节**：

```assembly
# Trap 入口（简化版）
trap_entry:
    mv t0, sp                      # 保存原始 sp
    csrrw sp, sscratch, sp          # 交换 sp 和 sscratch（切换到内核栈）
    addi sp, sp, -272              # 分配 TrapFrame
    sd t0, 0(sp)                   # 保存原始 sp
    # ... 保存寄存器 ...
    call trap_handler              # 调用 Rust 处理函数
    # ... 恢复寄存器 ...
    ld t0, 0(sp)                   # 加载原始 sp
    addi sp, sp, 272               # 释放 TrapFrame
    csrr t1, sscratch              # 读取内核栈指针
    mv sp, t0                      # 恢复原始 sp
    csrw sscratch, t1              # 恢复内核栈指针到 sscratch
    sret                           # 返回用户/内核模式
```

**已验证的系统调用**：
- ✅ SYS_EXIT (93) - 进程退出
- ✅ SYS_GETPID (172) - 获取进程 ID

**测试结果**：
```
[TRAP:ECALL]           <- 陷阱处理入口
[ECALL:5D]             <- 系统调用 0x5D (93) = sys_exit
sys_exit: exiting with code 0  <- sys_exit 执行成功
]                      <- 汇编代码到达 sret
```

**关键文件**：
- `kernel/src/arch/riscv64/trap.S` - Trap 入口/出口汇编代码
- `kernel/src/arch/riscv64/trap.rs` - Trap 处理 Rust 代码
- `kernel/src/arch/riscv64/syscall.rs` - 系统调用分发和实现
- `kernel/src/arch/riscv64/usermode_asm.S` - 用户模式切换汇编
- `kernel/src/embedded_user_programs.rs` - 嵌入的用户程序 ELF 数据

#### 🐛 Bug 修复

**sscratch 寄存器管理问题**
- **问题**：在 trap 出口时，用户栈指针被错误地写入 sscratch
- **影响**：第二个系统调用无法正确切换到内核栈
- **修复**：在 trap 出口时重新加载内核栈指针到 sscratch
- **代码**：
```assembly
ld t0, 0(sp)           # Load original sp (user or kernel)
addi sp, sp, 272       # Deallocate trap frame
csrr t1, sscratch      # Read kernel stack pointer from sscratch
mv sp, t0              # Restore original sp (user or kernel)
csrw sscratch, t1      # Restore kernel stack pointer to sscratch
```

**用户程序嵌入更新问题**
- **问题**：修改用户程序后没有重新嵌入到内核
- **影响**：内核运行旧版本的用户程序
- **修复**：使用 `embed_user_programs.sh` 脚本重新嵌入用户程序 ELF

#### 📝 文档更新

- 添加 RISC-V 系统调用实现文档
- 更新用户程序开发指南
- 添加 trap 处理流程图

### 2025-02-08

#### 🐛 Bug 修复

**BuddyAllocator 伙伴地址越界修复** (commit 09c86dd)
- 修复 `free_blocks` 函数在合并伙伴块时的地址越界问题
- 添加伙伴地址边界检查，防止访问超出 heap_end 的内存
- 影响：解决了 FdTable 和 SimpleArc 测试的 Page Fault 问题

**问题描述**：
- 释放 order 12 (16MB) 块时，伙伴地址计算为 0x81A00000
- 这个地址正好是 heap_end，超出 MMU 映射范围
- 导致 Load page fault 错误

**修复方案**：
```rust
// 检查伙伴是否在堆范围内
let heap_start = self.heap_start.load(Ordering::Acquire);
let heap_end = self.heap_end.load(Ordering::Acquire);

if buddy_ptr < heap_start || buddy_ptr >= heap_end {
    // 伙伴超出堆范围，无法合并
    self.add_to_free_list(current_ptr as *mut BlockHeader, current_order);
    break;
}
```

**测试验证**：
- ✅ SimpleArc 分配测试成功
- ✅ FdTable 测试成功
- ✅ 不再有 Page Fault 错误

#### ✨ 新增

**SimpleArc 分配测试** (kernel/src/tests/arc_alloc.rs)
- 新增 SimpleArc 内存分配和释放测试
- 验证 Arc::clone、引用计数、drop 功能
- 测试 File 对象的创建和访问

#### 📝 文档更新

- 重构文档结构，创建清晰的分类组织
- 添加文档中心索引 (docs/README.md)
- 归档历史调试文档到 docs/archive/

---

## [0.1.0] - 2025-02-08

#### ✨ 新功能

**Unix 进程管理系统调用** (Phase 15)
- ✅ fork() - 创建子进程 (commit a4bbc7a)
- ✅ execve() - 执行新程序 (commit 3b5f96d)
- ✅ wait4() - 等待子进程 (commit 22ab972)

**同步原语** (Phase 14)
- ✅ Semaphore - 信号量机制 (commit 5ea2376)
- ✅ Condition Variable - 条件变量 (commit e832be1)

**RISC-V 架构支持** (Phase 10)
- ✅ 启动流程和 OpenSBI 集成
- ✅ Sv39 MMU 和页表管理
- ✅ PLIC 中断控制器驱动
- ✅ IPI 核间中断框架
- ✅ SMP 多核支持 (SBI HSM)

#### 🐛 Bug 修复

**内核启动问题** (commit 9de7b64)
- 修复内核启动时的 OpenSBI 集成问题
- 修复 wait4 错误码处理

**Timer interrupt sepc 处理**
- 不再跳过 WFI 指令，避免跳转到指令中间

**SMP + MMU 竞态条件**
- 使用 `AtomicUsize` 保护 `alloc_page_table()` 的 `NEXT_INDEX`
- Per-CPU MMU 使能：次核等待启动核完成页表初始化

#### 📊 测试覆盖

- ✅ 14 个单元测试模块
- ✅ fork、execve、wait4 测试
- ✅ SMP 多核启动测试
- ✅ SimpleArc 和 FdTable 测试

#### 📝 文档

- CLAUDE.md - AI 辅助开发指南
- UNIT_TEST.md - 单元测试文档
- USER_PROGRAMS.md - 用户程序实现方案
- CODE_REVIEW.md - 代码审查记录

---

## 版本命名规则

- **Major.Minor.Patch** (主版本.次版本.补丁)
- Major：重大架构变更或不兼容更新
- Minor：新功能添加
- Patch：Bug 修复和小改进

## 提交规范

遵循 [Conventional Commits](https://www.conventionalcommits.org/)：

- `feat:` - 新功能
- `fix:` - Bug 修复
- `docs:` - 文档更新
- `test:` - 测试相关
- `refactor:` - 代码重构
- `perf:` - 性能优化
