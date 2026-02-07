# Rux 用户程序构建系统

## 概述

这个目录包含 Rux 内核的用户空间程序。所有程序编译为独立的 ELF 二进制文件，可以通过内核的 ELF 加载器和 execve 系统调用执行。

## 目录结构

```
userspace/
├── Cargo.toml              # 工作空间配置
├── .cargo/
│   └── config.toml         # RISC-V 交叉编译配置
├── user.ld                 # 用户程序链接器脚本
├── Makefile                # 构建自动化
├── README.md               # 本文件
└── hello_world/            # Hello World 示例程序
    ├── Cargo.toml
    └── src/
        └── main.rs
```

## 快速开始

### 前置要求

- Rust 工具链（stable）
- RISC-V target：`riscv64gc-unknown-none-elf`

### 安装 RISC-V 目标

```bash
rustup target add riscv64gc-unknown-none-elf
```

### 构建用户程序

```bash
# 进入用户程序目录
cd userspace

# 构建所有程序
make

# 构建特定程序
make hello_world

# 清理构建产物
make clean

# 查看帮助
make help
```

### 输出位置

构建的二进制文件位于：
```
target/riscv64gc-unknown-none-elf/release/<program_name>
```

例如：
```
target/riscv64gc-unknown-none-elf/release/hello_world
```

## 程序示例

### hello_world

最简单的用户程序，演示：

- no_std 环境编程
- 系统调用接口使用
- 字符串输出

**源代码**：`hello_world/src/main.rs`

**功能**：打印 "Hello, World!" 并退出

**系统调用**：
- `SYS_WRITE (64)` - 写入字符串
- `SYS_EXIT (93)` - 退出程序

## 添加新程序

### 1. 创建程序目录

```bash
mkdir userspace/my_program
cd userspace/my_program
mkdir src
```

### 2. 创建 Cargo.toml

```toml
[package]
name = "my_program"
version.workspace = true
edition.workspace = true

[profile.dev]
panic = "abort"

[profile.release]
panic = "abort"
opt-level = "z"
lto = true
codegen-units = 1
```

### 3. 创建源代码

```rust
// src/main.rs
#![no_std]
#![no_main]

use core::arch::asm;
use core::panic::PanicInfo;

// ... 你的程序代码 ...

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 程序入口点
    loop {}
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
```

### 4. 更新工作空间

在 `userspace/Cargo.toml` 中添加：

```toml
[workspace]
members = [
    "hello_world",
    "my_program",  # 添加这一行
]
```

### 5. 更新 Makefile

在 `userspace/Makefile` 中添加：

```makefile
# 为每个用户程序定义构建规则
$(eval $(call BUILD_USER_PROGRAM,hello_world))
$(eval $(call BUILD_USER_PROGRAM,my_program))  # 添加这一行
```

### 6. 构建新程序

```bash
make my_program
```

## 系统调用接口

用户程序使用 RISC-V Linux ABI 的系统调用约定：

### 系统调用执行

```rust
pub unsafe fn syscall1(n: u64, a0: u64) -> u64 {
    let mut ret: u64;
    asm!(
        "ecall",
        inlateout("a7") n => _,
        inlateout("a0") a0 => ret,
        // ...
    );
    ret
}
```

### 常用系统调用号

```rust
const SYS_WRITE: u64 = 64;   // write(fd, buf, count)
const SYS_EXIT: u64 = 93;    // exit(exit_code)
const SYS_READ: u64 = 63;    // read(fd, buf, count)
const SYS_OPEN: u64 = 1024;  // open(pathname, flags, mode)
const SYS_CLOSE: u64 = 57;   // close(fd)
```

### 寄存器约定

- **a7**: 系统调用号
- **a0-a5**: 参数（最多 6 个）
- **a0**: 返回值

## 内存布局

用户程序使用链接器脚本 `user.ld` 定义内存布局：

```
ORIGIN = 0x10000 (虚拟地址)
├── .text    - 代码段
├── .rodata  - 只读数据
├── .data    - 初始化数据
├── .bss     - 未初始化数据
└── .stack   - 栈空间（16KB）
```

**注意**：实际加载地址由内核的 ELF 加载器决定。

## 调试

### 检查 ELF 文件

```bash
# 查看文件信息
file target/riscv64gc-unknown-none-elf/release/hello_world

# 查看程序大小
ls -lh target/riscv64gc-unknown-none-elf/release/hello_world

# 使用 readelf 查看详细信息
riscv64-unknown-elf-readelf -h target/riscv64gc-unknown-none-elf/release/hello_world

# 反汇编
riscv64-unknown-elf-objdump -d target/riscv64gc-unknown-none-elf/release/hello_world
```

### QEMU 测试

**注意**：当前需要内核的 ELF 加载器和 execve 系统调用支持（Phase 11.2-11.3）。

```bash
# 使用 QEMU 用户模式（需要完整的用户空间支持）
qemu-riscv64 -L /usr/riscv64-linux-gnu target/riscv64gc-unknown-none-elf/release/hello_world
```

## 限制

当前已知限制：

1. **无标准库支持**
   - 无法使用 `std` 库
   - 需要手动实现所有功能

2. **无动态链接**
   - 静态链接所有代码
   - 不支持共享库

3. **有限的系统调用**
   - 依赖内核实现的系统调用
   - 当前 Phase 11.2-11.3 开发中

## 下一步

- [ ] Phase 11.2：实现 ELF 加载器
- [ ] Phase 11.3：实现 execve 系统调用
- [ ] Phase 11.4：测试和验证

## 参考资料

- [RISC-V Linux ABI](https://github.com/riscv-non-isa/riscv-elf-psabi-doc)
- [Linux 系统调用表](https://man7.org/linux/man-pages/man2/syscalls.2.html)
- [ELF 格式规范](https://refspecs.linuxfoundation.org/elf/elf.pdf)
- [docs/USER_PROGRAMS.md](../docs/USER_PROGRAMS.md)
