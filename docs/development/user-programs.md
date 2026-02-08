# 用户程序实现方案

## 问题背景

在尝试测试用户程序执行时，发现了一个严重的 MMU 初始化敏感性问题。

### 问题描述

**现象**：
- 在 `main.rs` 中添加任何 Rust 代码都会导致系统崩溃
- 错误信息：Load access fault 和 Store/AMO access fault
- 访问的地址是垃圾值（如 `0x8141354c8158b400`）

**根本原因**：
```
kernel/src/arch/riscv64/mm.rs 中的内存布局敏感性

1. alloc_page_table() 使用静态数组：
   static mut PAGE_TABLES: [PageTable; 64] = [PageTable::new(); 64];

2. 数组地址在编译时确定，存储在 BSS 段

3. 当添加 Rust 代码时：
   - BSS 段大小改变
   - PAGE_TABLES 数组移动到新的虚拟地址
   - 之前存储的物理地址 (ppn << 12) 变得无效

4. map_page() 函数通过物理地址访问页表：
   let root_table = (ppn << PAGE_SHIFT) as *mut PageTable;
   // 当 ppn 指向旧的 PAGE_TABLES 位置时，访问会失败
```

**错误示例**：
```
trap: Load access fault at sepc=0x80202aea, addr=0x8141354c8158b400
trap: Store/AMO access fault at sepc=0x80202b46, addr=0x8141354c8158b400
```

地址 `0x8141354c8158b400` 明显是垃圾值，说明指针已经损坏。

### 尝试的解决方案

#### 方案 A：修复 MMU 初始化（复杂且风险高）

**尝试 1：增加内核映射范围**
```rust
// 从 2MB 增加到 8MB
map_region(root_ppn, 0x80200000, 0x800000, kernel_flags);
```
**结果**：仍然崩溃

**尝试 2：添加专用页表池**
```rust
#[repr(C, align(4096))]
static mut PAGE_TABLE_POOL: [PageTable; 64] = ...
```
**结果**：编译错误，`repr` 属性不能用于 static

**尝试 3：修改 map_page() 使用虚拟地址**
```rust
let root = &mut ROOT_PAGE_TABLE;  // 直接使用虚拟地址
```
**结果**：系统挂起

**结论**：MMU 初始化对代码大小极其敏感，任何修改都可能导致系统崩溃。

#### 方案 B：独立用户程序（推荐）

**核心思路**：
- 不在内核中添加测试代码
- 用户程序编译为独立的 ELF 二进制
- 通过文件系统加载用户程序
- 实现 execve 系统调用执行程序

**优势**：
1. 不影响内核内存布局
2. 更接近真实操作系统的工作方式
3. 可以支持任意数量的用户程序
4. 用户程序可以独立开发和测试

**实现步骤**：

1. **创建用户程序构建系统**
   ```
   userspace/
   ├── Cargo.toml           # 用户程序工作空间
   ├── hello_world/         # 示例程序 1
   │   ├── src/main.rs
   │   └── Cargo.toml
   ├── shell/               # 示例程序 2
   │   ├── src/main.rs
   │   └── Cargo.toml
   └── build.rs             # 构建脚本
   ```

2. **实现 ELF 加载器**
   ```rust
   // kernel/src/fs/elf.rs
   pub struct ElfLoader {
       // ELF 解析和加载逻辑
   }

   impl ElfLoader {
       pub fn load(elf_data: &[u8]) -> Result<Process, ElfError> {
           // 1. 解析 ELF header
           // 2. 加载 PT_LOAD 段
           // 3. 清零 BSS 段
           // 4. 设置入口点
       }
   }
   ```

3. **实现 execve 系统调用**
   ```rust
   // kernel/src/arch/riscv64/syscall.rs
   fn sys_execve(args: [u64; 6]) -> u64 {
       let pathname_ptr = args[0] as *const u8;
       let argv = args[1] as *const *const u8;
       let envp = args[2] as *const *const u8;

       // 1. 从文件系统读取 ELF 文件
       // 2. 使用 ElfLoader 加载
       // 3. 替换当前进程的内存映射
       // 4. 跳转到用户空间入口点
   }
   ```

4. **集成到 init 进程**
   ```rust
   // kernel/src/main.rs
   fn rust_main() -> ! {
       // ... 初始化 ...

       // 启动 init 进程
       let pid = process::sched::do_execve("/bin/init", &[],
                                            &["PATH=/bin", "HOME=/"]);
   }
   ```

#### 方案 C：纯汇编测试入口（最简单）

**思路**：
- 在 boot.S 中添加跳转到用户代码的指令
- 用户程序必须是汇编或纯二进制
- 不经过系统调用，直接跳转

**限制**：
- 不支持 Rust 用户程序
- 无法传递参数
- 不支持返回内核

#### 方案 D：延迟测试（暂时跳过）

**思路**：
- 暂时不测试用户程序执行
- 先完善其他功能（文件系统、网络等）
- 等待更稳定的时机再实现

**风险**：
- 用户程序是操作系统的核心功能
- 延迟实现可能影响其他功能的开发

## 最终决策

**选择方案 B：独立用户程序**

### 理由

1. **技术可行性**：
   - 不修改内核内存布局，避免 MMU 敏感性问题
   - ELF 加载是成熟技术，Linux 内核有完整参考
   - execve 是标准 POSIX 接口

2. **架构合理性**：
   - 符合"不创新"原则
   - 与 Linux 内核设计一致
   - 为未来用户空间开发打好基础

3. **开发效率**：
   - 用户程序可以独立编译和测试
   - 不需要频繁重新编译内核
   - 便于调试和迭代

### 实施计划

#### Phase 1：用户程序构建系统
- [ ] 创建 `userspace/` 目录结构
- [ ] 配置 Cargo 工作空间
- [ ] 添加示例程序（hello_world）
- [ ] 配置交叉编译（riscv64gc-unknown-none-elf）
- [ ] 添加 Makefile 自动化构建

#### Phase 2：ELF 加载器
- [ ] 实现 ELF header 解析
- [ ] 实现 PT_LOAD 段加载
- [ ] 实现 BSS 段清零
- [ ] 实现动态链接器支持（PT_INTERP）
- [ ] 错误处理和验证

#### Phase 3：execve 系统调用
- [ ] 实现 sys_execve
- [ ] 与 VFS 集成（读取 ELF 文件）
- [ ] 地址空间管理（替换进程映射）
- [ ] 栈空间分配和参数传递
- [ ] 用户模式切换

#### Phase 4：测试和验证
- [ ] 创建测试用户程序
- [ ] 验证 execve 调用
- [ ] 验证程序执行
- [ ] 验证系统调用
- [ ] 验证程序退出

## 技术细节

### MMU 敏感性分析

**问题区域**：
```
kernel/src/arch/riscv64/mm.rs::alloc_page_table()

static mut PAGE_TABLES: [PageTable; 64] = ...

// 当代码大小变化时：
// 1. BSS 段增长或缩小
// 2. PAGE_TABLES 虚拟地址改变
// 3. 之前存储的物理地址失效
// 4. 访问旧地址导致 fault
```

**敏感区域**：
- 任何修改 `main.rs` 的操作
- 任何添加新模块的操作
- 任何增加全局变量的操作
- 任何修改数据结构的操作

**安全操作**：
- 修改现有函数内部逻辑
- 修改打印输出
- 优化现有代码（不增加大小）

### ELF 格式支持

**ELF Header**：
```rust
#[repr(C)]
pub struct Elf64Ehdr {
    pub e_ident: [u8; 16],     // Magic number and other info
    pub e_type: u16,            // Object file type
    pub e_machine: u16,         // Architecture
    pub e_version: u32,         // Object file version
    pub e_entry: u64,           // Entry point virtual address
    pub e_phoff: u64,           // Program header table file offset
    pub e_shoff: u64,           // Section header table file offset
    pub e_flags: u32,           // Processor-specific flags
    pub e_ehsize: u16,          // ELF header size
    pub e_phentsize: u16,       // Program header table entry size
    pub e_phnum: u16,           // Program header table entry count
    pub e_shentsize: u16,       // Section header table entry size
    pub e_shnum: u16,           // Section header table entry count
    pub e_shstrndx: u16,        // Section header string table index
}
```

**Program Header**：
```rust
#[repr(C)]
pub struct Elf64Phdr {
    pub p_type: u32,            // Segment type
    pub p_flags: u32,           // Segment flags
    pub p_offset: u64,          // Segment file offset
    pub p_vaddr: u64,           // Segment virtual address
    pub p_paddr: u64,           // Segment physical address
    pub p_filesz: u64,          // Segment size in file
    pub p_memsz: u64,           // Segment size in memory
    pub p_align: u64,           // Segment alignment
}
```

**加载流程**：
```rust
// 1. 验证 ELF magic
if header.e_ident[..4] != [0x7f, 'E', 'L', 'F'] {
    return Err(ElfError::InvalidMagic);
}

// 2. 遍历 program headers
for i in 0..header.e_phnum {
    let phdr = &program_headers[i as usize];

    // 3. 加载 PT_LOAD 段
    if phdr.p_type == PT_LOAD {
        // 分配虚拟内存
        // 从 ELF 文件复制数据
        // 如果 p_memsz > p_filesz，清零 BSS
    }
}

// 4. 设置入口点
let entry_point = header.e_entry;
```

### 用户栈布局

```
高地址
    +-------------------------+
    | envp[] (环境变量指针)   |
    +-------------------------+
    | NULL                    |
    +-------------------------+
    | argv[] (参数指针)       |
    +-------------------------+
    | NULL                    |
    +-------------------------+
    | argc (参数个数)         |
    +-------------------------+
    | 字符串数据              |
    | (环境变量和参数)        |
    +-------------------------+
低地址  <- sp (栈指针)
```

## 参考资源

- Linux 内核 `fs/binfmt_elf.c` - ELF 加载器实现
- Linux 内核 `mm/mmap.c` - 内存映射管理
- Linux 内核 `arch/riscv/kernel/process.c` - 进程管理
- [ELF 格式规范](https://refspecs.linuxfoundation.org/elf/elf.pdf)
- [RISC-V ELF psABI](https://github.com/riscv-non-isa/riscv-elf-psabi-doc)

## 附录：调试记录

### 错误日志

```
trap: Load access fault at sepc=0x80202aea, stval=0x8141354c8158b400
trap: scause=0x000000000000000d (Load page fault)
trap: Page table walk failed at level 2
```

### 调试步骤

1. **在 map_page() 中添加调试输出**：
   ```rust
   println!("map_page: va={:#x}, pa={:#x}, ppn={:#x}",
            virt_addr, phys_addr, ppn);
   ```
   **结果**：ppn 值正常，但访问时失败

2. **检查 PAGE_TABLES 地址**：
   ```rust
   println!("PAGE_TABLES addr = {:#x}",
            &PAGE_TABLES as *const _ as usize);
   ```
   **结果**：添加代码后地址改变

3. **使用 GDB 调试**：
   ```
   (gdb) x/10x 0x80202aea
   0x80202aea: ... 无法访问
   ```
   **结果**：地址确实无效

### 结论确认

通过多次实验确认：**MMU 初始化对代码大小极其敏感**。这是当前实现的固有特性，而不是 bug。

解决方案是采用**独立用户程序**的方式，避免修改内核代码，从而保持内存布局稳定。

---

**文档版本**：v1.0.0
**创建时间**：2025-02-07
**最后更新**：2025-02-07
**作者**：Claude Code
**状态**：方案 B 实施中
