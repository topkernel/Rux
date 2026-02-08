# 开发流程规范 (Development Workflow)

本文档记录 Rux 内核开发的标准流程，确保每次代码修改都经过完整的验证和文档更新。

## 标准开发流程

### 1. 编写代码 (Write Code)

**原则**：
- 遵循 [DESIGN.md](DESIGN.md) 中的设计原则
- 完全遵循 Linux ABI/POSIX 标准（见 [CLAUDE.md](../CLAUDE.md)）
- 参考 Linux 内核源码实现

**步骤**：
1. 阅读 Linux 内核相关代码
2. 理解 POSIX 标准要求
3. 实现 Rust 代码
4. 添加必要的注释和文档

### 2. 单元测试 (Unit Tests)

**测试内容**：
- 函数级别的逻辑验证
- 边界条件测试
- 错误路径测试

**实现方式**：
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_function_name() {
        // 测试代码
        assert_eq!(result, expected);
    }
}
```

**示例位置**：
- `kernel/src/fs/rootfs.rs:1030-1060` - RootFS 单元测试

### 3. 集成测试 (Integration Tests)

**测试内容**：
- 多个模块协作验证
- 系统级功能测试
- 内核启动后执行

**实现方式**：
在 `kernel/src/main.rs` 中添加测试函数：
```rust
#[cfg(feature = "riscv64")]
fn test_feature_name() {
    println!("test: Testing feature...");

    // 测试代码
    // 验证结果
}
```

在 `main()` 函数中调用：
```rust
// 系统就绪后执行测试
#[cfg(feature = "riscv64")]
test_feature_name();
```

**示例位置**：
- `kernel/src/main.rs:333-472` - file_open() 集成测试
- `kernel/src/main.rs:148-331` - shell 执行测试

### 4. 整机测试 (Full System Testing)

**测试目标**：
- 验证内核正常启动
- 验证多核支持（SMP）
- 验证功能在真实环境中工作

**测试命令**：
```bash
# 编译
make build

# 单核启动测试
timeout 3 qemu-system-riscv64 -M virt -cpu rv64 -m 2G \
  -nographic -serial mon:stdio \
  -kernel target/riscv64gc-unknown-none-elf/debug/rux

# 多核启动测试
timeout 3 qemu-system-riscv64 -M virt -cpu rv64 -m 2G \
  -nographic -serial mon:stdio -smp 4 \
  -kernel target/riscv64gc-unknown-none-elf/debug/rux

# 使用测试脚本
bash test/run.sh
```

**验证要点**：
- [ ] 内核成功启动
- [ ] 所有 hart 初始化（多核模式）
- [ ] 测试输出正确
- [ ] 无 panic 或挂起

### 5. 更新文档 (Update Documentation)

**需要更新的文档**：

1. **代码审查记录** ([CODE_REVIEW.md](CODE_REVIEW.md))
   - 标记已修复的问题为 ✅
   - 记录修复方案和提交信息
   - 更新待修复问题列表

2. **任务列表** ([TODO.md](TODO.md))
   - 标记已完成的任务
   - 添加新发现的任务
   - 更新进度

3. **设计文档** (如适用)
   - [DESIGN.md](DESIGN.md) - 架构设计变更
   - [STRUCTURE.md](STRUCTURE.md) - 目录结构变更
   - [QUICKREF.md](QUICKREF.md) - 快速参考更新

4. **新增文档** (如适用)
   - 新功能的说明文档
   - 调试指南
   - 测试指南

### 6. 提交代码 (Commit Code)

**提交前检查**：
```bash
# 查看修改
git status
git diff

# 编译验证
make build

# 运行测试
bash test/run.sh
```

**提交规范**：
```bash
git add <files>
git commit -m "<type>: <description>

## 详细说明

### 修改内容
- 具体修改点 1
- 具体修改点 2

### 技术细节
- 技术说明
- 设计决策

### 验证
- ✅ 测试 1 通过
- ✅ 测试 2 通过

### 相关文件
- file1.rs
- file2.rs

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

**提交类型**：
- `feat`: 新功能
- `fix`: 错误修复
- `test`: 测试相关
- `docs`: 文档更新
- `refactor`: 代码重构
- `perf`: 性能优化
- `chore`: 构建/工具链相关

**示例提交**：
```bash
git commit -m "feat: 实现 VFS file_open() 函数

## 实现内容

### 1. file_open() 函数
- 使用 RootFS::lookup() 查找文件
- 支持 O_CREAT/O_EXCL/O_TRUNC 标志
- 创建 File 对象并分配文件描述符

### 2. RootFS 文件操作
- rootfs_file_read() - 文件读取
- rootfs_file_write() - 文件写入
- rootfs_file_lseek() - 文件定位

## 验证
- ✅ 编译成功
- ✅ 多核启动测试通过
- ✅ 集成测试通过

Co-Authored-By: Claude Sonnet 4.5 <noreply@anthropic.com>"
```

## 快速检查清单

在提交任何代码前，确保：

- [ ] **代码编译通过** (`make build`)
- [ ] **单元测试通过** (如适用)
- [ ] **集成测试通过** (如适用)
- [ ] **多核启动测试通过** (`bash test/run.sh`)
- [ ] **文档已更新** (CODE_REVIEW.md, TODO.md 等)
- [ ] **提交信息清晰** (遵循提交规范)
- [ ] **遵循 Linux ABI** (不创新标准)
- [ ] **代码审查完成** (自我审查或同行审查)

## 常见错误

### ❌ 错误做法

1. **只编译不测试**
   - 编译通过 ≠ 功能正确
   - 必须运行测试验证

2. **跳过文档更新**
   - CODE_REVIEW.md 中的问题未标记
   - TODO.md 未更新进度
   - 未来无法追踪问题状态

3. **提交信息不清晰**
   - "fix bug" - 太简略
   - "update" - 无具体内容
   - 应该说明修改了什么、为什么、如何验证

4. **违反"不创新"原则**
   - 自己设计接口
   - 修改 Linux 标准行为
   - 必须完全兼容 Linux ABI

### ✅ 正确做法

1. **完整测试流程**
   ```bash
   make build           # 编译
   make test           # 运行测试
   # 或手动验证功能
   ```

2. **及时更新文档**
   - 每次修复问题后更新 CODE_REVIEW.md
   - 完成功能后更新 TODO.md
   - 重大变更更新 DESIGN.md

3. **清晰提交信息**
   ```
   type: 简短描述（50 字符内）

   ## 详细说明
   - 修改点 1
   - 修改点 2

   ## 验证
   - ✅ 测试通过

   Co-Authored-By: Claude Sonnet 4.5
   ```

4. **严格遵循标准**
   - 参考 Linux 内核源码
   - 使用 Linux 系统调用号
   - 遵循 POSIX 标准

## 示例：完整开发流程

### 任务：实现 file_open() 函数

**1. 编写代码**
- 阅读 Linux fs/open.c
- 理解 do_sys_openat 行为
- 实现 vfs::file_open()

**2. 单元测试**
- 在 rootfs.rs 中添加 `#[test]`
- 验证 lookup 和 create_file

**3. 集成测试**
- 在 main.rs 中添加 test_file_open()
- 测试各种 flag 组合

**4. 整机测试**
```bash
make build
bash test/run.sh
# 验证测试输出
```

**5. 更新文档**
- 更新 CODE_REVIEW.md（标记问题状态）
- 更新 TODO.md（进度）
- 创建 DEVELOPMENT_WORKFLOW.md（本文档）

**6. 提交代码**
```bash
git add kernel/src/fs/vfs.rs kernel/src/main.rs docs/
git commit -m "feat: 实现 VFS file_open() 函数

..."
```

## 相关文档

- [CLAUDE.md](../CLAUDE.md) - AI 助手开发指南
- [DESIGN.md](DESIGN.md) - 设计原则
- [CODE_REVIEW.md](CODE_REVIEW.md) - 代码审查记录
- [TODO.md](TODO.md) - 任务列表
- [QUICKREF.md](QUICKREF.md) - 快速参考

## 版本历史

- **2026-02-08**: 创建文档，记录标准开发流程
  - 添加 6 步开发流程
  - 添加快速检查清单
  - 添加常见错误示例
  - 添加完整开发流程示例
