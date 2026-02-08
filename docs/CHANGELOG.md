# Rux OS 变更日志

本文档记录 Rux 内核的重要变更和修复。

## [Unreleased]

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

- 更新 README.md：添加 BuddyAllocator 修复记录
- 更新 TODO.md：记录 Phase 15.5 修复内容
- 更新 CODE_REVIEW.md：详细的修复说明和对比 Linux

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
