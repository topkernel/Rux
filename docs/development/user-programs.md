# 用户程序开发指南

本文档说明如何在 Rux OS 中开发和运行用户程序。

**最后更新**：2026-02-15
**状态**：✅ Shell 成功运行，支持 no_std 和 musl libc 程序

---

## 目录

- [概述](#概述)
- [用户程序类型](#用户程序类型)
- [Shell 选择](#shell-选择)
- [no_std 用户程序](#no_std-用户程序)
- [musl libc 程序](#musl-libc-程序)
- [系统调用](#系统调用)
- [调试技巧](#调试技巧)
- [已知限制](#已知限制)

---

## 概述

Rux OS 支持 RISC-V 64 位用户程序，通过以下机制：

1. **ELF 加载器** - 解析和加载 ELF 格式的用户程序
2. **用户模式切换** - 使用 sret 指令从 S-mode 切换到 U-mode
3. **系统调用处理** - 使用 ecall 指令从用户模式进入内核
4. **单一页表方法** - Linux 风格，通过 U-bit 控制权限

---

## 用户程序类型

Rux OS 支持三种类型的用户程序：

| 类型 | 状态 | 描述 |
|------|------|------|
| **no_std Rust** | ✅ 完全可用 | 裸机 Rust 程序，无标准库 |
| **musl libc C** | ⏳ 部分支持 | C 程序，需要 argc/argv 初始化 |
| **Rust std** | ⏳ 部分支持 | Rust 标准库程序，需要 argc/argv 初始化 |

### no_std 用户程序（推荐）

默认 Shell 是 no_std Rust 实现，完全可用：

```bash
make run  # 默认使用 no_std shell
```

### musl libc 程序

需要 musl libc 工具链，目前需要修复 argc/argv 初始化：

```bash
make run SHELL_TYPE=cshell
```

---

## Shell 选择

通过 Makefile 参数选择不同的 Shell：

```bash
# 默认 no_std shell（推荐）
make run SHELL_TYPE=default

# C musl shell（需要修复）
make run SHELL_TYPE=cshell

# Rust std shell（需要修复）
make run SHELL_TYPE=rust-shell
```

---

## no_std 用户程序

### 最小示例

**文件**：`userspace/hello_world/src/main.rs`

```rust
#!/usr/bin/env rust-script
//! Rux 用户程序示例 - Hello World

#![no_std]
#![no_main]

use core::panic::PanicInfo;

// ============================================================================
// 系统调用接口（RISC-V Linux ABI）
// ============================================================================

mod syscall {
    /// 系统调用号（遵循 RISC-V Linux ABI）
    pub const SYS_EXIT: u64 = 93;

    /// 执行系统调用（1个参数）
    #[inline(always)]
    pub unsafe fn syscall1(n: u64, a0: u64) -> u64 {
        let mut ret: u64;
        core::arch::asm!(
            "ecall",
            inlateout("a7") n => _,
            inlateout("a0") a0 => ret,
            lateout("a1") _,
            lateout("a2") _,
            lateout("a3") _,
            lateout("a4") _,
            lateout("a5") _,
            lateout("a6") _,
            options(nostack, nomem)
        );
        ret
    }
}

// ============================================================================
// 程序入口点
// ============================================================================

/// 用户程序入口点
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 调用 sys_exit(0) 退出程序
    unsafe { syscall::syscall1(syscall::SYS_EXIT, 0) };

    // 如果 sys_exit 失败，进入死循环
    loop {
        unsafe { core::arch::asm!("nop", options(nomem, nostack)) };
    }
}

// ============================================================================
// Panic 处理
// ============================================================================

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        unsafe { core::arch::asm!("nop", options(nomem, nostack)) };
    }
}
```

---

## musl libc 程序

### 构建工具链

```bash
cd /home/william/Rux/toolchain
bash build-musl.sh
```

### C Shell 示例

**文件**：`userspace/cshell/src/shell.c`

```c
#include <unistd.h>
#include <stdio.h>
#include <string.h>

int main(int argc, char *argv[]) {
    printf("Rux OS Shell v0.2 (musl libc)\n");

    while (1) {
        printf("rux> ");
        fflush(stdout);

        char cmd[256];
        if (fgets(cmd, sizeof(cmd), stdin) == NULL) {
            break;
        }

        // 处理命令...
    }

    return 0;
}
```

### musl 链接器脚本

**文件**：`userspace/musl.ld`

用户空间程序内存布局：
- TEXT: 0x10000 (1MB)
- DATA: 0x110000 (512KB)
- HEAP: 0x190000 (2MB)
- STACK: 0x390000 (128KB)

---

## 系统调用

### 系统调用约定

**寄存器约定**（RISC-V Linux ABI）：
- `a7`: 系统调用号
- `a0-a5`: 参数（最多 6 个）
- `a0`: 返回值

### 已实现的系统调用

| 系统调用号 | 名称 | 状态 |
|-----------|------|------|
| 63 | sys_read | ✅ |
| 64 | sys_write | ✅ |
| 56 | sys_openat | ✅ |
| 57 | sys_close | ✅ |
| 93 | sys_exit | ✅ |
| 172 | sys_getpid | ✅ |
| 110 | sys_getppid | ✅ |
| 214 | sys_brk | ✅ |

---

## 调试技巧

### 1. 添加调试输出

```rust
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 调试：写入字符到 UART
    unsafe {
        const UART: u64 = 0x10000000;
        core::ptr::write_volatile(UART as *mut u8, b'H');
        core::ptr::write_volatile(UART as *mut u8, b'i');
    }

    syscall1(93, 0);
    loop { core::arch::asm!("nop", options(nomem, nostack)); }
}
```

### 2. 查看系统调用返回值

```rust
let pid = syscall1(172, 0);
// 根据 pid 值采取不同行动
```

---

## 已知限制

### 当前限制

1. **libc 程序启动** - musl libc 和 Rust std 程序需要 argc/argv 栈初始化
2. **文件系统** - 部分系统调用已实现，完整支持待完善

### 待修复问题

**cshell/rust-shell 启动失败**
- 原因：musl libc 的 `__init_libc` 期望从栈读取 argc/argv
- 当前：UserContext::new() 初始化所有寄存器为 0
- 解决方案：需要在 UserContext 中设置 argc/argv 和栈初始化

---

## 参考资料

- [RISC-V Linux ABI](https://github.com/riscv-non-isa/riscv-elf-psabi-doc)
- [RISC-V 特权级架构规范](https://riscv.org/specifications/privileged-isa/)
- [ELF 格式规范](https://refspecs.linuxfoundation.org/elf/elf.pdf)
- [Linux 系统调用表](https://github.com/torvalds/linux/blob/master/arch/riscv/include/asm/unistd.h)

## 目录

- [概述](#概述)
- [用户程序结构](#用户程序结构)
- [编译用户程序](#编译用户程序)
- [嵌入用户程序](#嵌入用户程序)
- [系统调用](#系统调用)
- [调试技巧](#调试技巧)
- [已知限制](#已知限制)

---

## 概述

Rux OS 支持 RISC-V 64 位用户程序，通过以下机制：

1. **ELF 加载器** - 解析和加载 ELF 格式的用户程序
2. **用户模式切换** - 使用 sret 指令从 S-mode 切换到 U-mode
3. **系统调用处理** - 使用 ecall 指令从用户模式进入内核
4. **单一页表方法** - Linux 风格，通过 U-bit 控制权限

### 用户程序执行流程

```
┌─────────────────────────────────────────────────────────────┐
│ 1. 内核加载用户程序 ELF 到内存                              │
│    - 解析 ELF 程序头                                        │
│    - 分配物理内存页                                         │
│    - 映射到用户虚拟地址空间                                  │
└─────────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────────┐
│ 2. 内核切换到用户模式                                       │
│    - 设置 sstatus.SPP=0 (返回 U-mode)                       │
│    - 设置 sepc=用户程序入口点                               │
│    - 设置 sp=用户栈指针                                     │
│    - 执行 sret                                              │
└─────────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────────┐
│ 3. 用户程序执行                                             │
│    - 在用户模式 (U-mode) 运行                              │
│    - 可以调用系统调用 (ecall)                               │
└─────────────────────────────────────────────────────────────┘
                          ↓
┌─────────────────────────────────────────────────────────────┐
│ 4. 系统调用处理                                             │
│    - ecall 指令触发异常                                    │
│    - 陷阱入口切换到内核栈                                  │
│    - 系统调用分发器执行相应功能                            │
│    - sret 返回用户模式                                      │
└─────────────────────────────────────────────────────────────┘
```

---

## 用户程序结构

### 最小示例

**文件**：`userspace/hello_world/src/main.rs`

```rust
#!/usr/bin/env rust-script
//! Rux 用户程序示例 - Hello World

#![no_std]
#![no_main]

use core::panic::PanicInfo;

// ============================================================================
// 系统调用接口（RISC-V Linux ABI）
// ============================================================================

mod syscall {
    /// 系统调用号（遵循 RISC-V Linux ABI）
    pub const SYS_EXIT: u64 = 93;

    /// 执行系统调用（1个参数）
    #[inline(always)]
    pub unsafe fn syscall1(n: u64, a0: u64) -> u64 {
        let mut ret: u64;
        core::arch::asm!(
            "ecall",
            inlateout("a7") n => _,
            inlateout("a0") a0 => ret,
            lateout("a1") _,
            lateout("a2") _,
            lateout("a3") _,
            lateout("a4") _,
            lateout("a5") _,
            lateout("a6") _,
            options(nostack, nomem)
        );
        ret
    }
}

// ============================================================================
// 程序入口点
// ============================================================================

/// 用户程序入口点
///
/// 注意：链接器会查找名为 `_start` 的符号作为入口点
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 调用 sys_exit(0) 退出程序
    unsafe { syscall::syscall1(syscall::SYS_EXIT, 0) };

    // 如果 sys_exit 失败，进入死循环
    loop {
        unsafe { core::arch::asm!("nop", options(nomem, nostack)) };
    }
}

// ============================================================================
// Panic 处理
// ============================================================================

/// Panic 处理函数
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        unsafe { core::arch::asm!("nop", options(nomem, nostack)) };
    }
}
```

### Cargo.toml 配置

**文件**：`userspace/hello_world/Cargo.toml`

```toml
[package]
name = "hello_world"
version = "0.1.0"
edition = "2021"

[dependencies]

# 注意：用户程序是 no_std 环境
# 不依赖标准库

[profile.release]
opt-level = "z"
lto = true
```

---

## 编译用户程序

### 方法 1：使用 build.sh 脚本（推荐）

```bash
cd /home/william/Rux/userspace
bash build.sh --bin hello_world
```

**输出**：
```
构建完成！输出文件：
  - target/riscv64gc-unknown-none-elf/release/hello_world (5208 bytes)
```

### 方法 2：手动编译

```bash
cd /home/william/Rux/userspace/hello_world

# 设置环境
export RUSTFLAGS="-C link-arg=-Tuser.ld"

# 编译
cargo build --release

# 查看输出
ls -lh target/riscv64gc-unknown-none-elf/release/hello_world
```

---

## 嵌入用户程序

Rux OS 使用嵌入方式加载用户程序（用于测试）。

### 嵌入脚本

**文件**：`kernel/embed_user_programs.sh`

```bash
#!/bin/bash
# 嵌入用户程序到内核源码

HELLO_WORLD="userspace/target/riscv64gc-unknown-none-elf/release/hello_world"
OUTPUT="kernel/src/embedded_user_programs.rs"

# 使用 xxd 将 ELF 转换为 Rust 字节数组
xxd -i "$HELLO_WORLD" | \
    awk 'BEGIN { print "pub static HELLO_WORLD_ELF: &[u8] = &[" }
         { printf "0x%s, ", $2 }
         END { print "\n];" }' > "$OUTPUT"

echo "✓ 用户程序已嵌入"
```

### 运行嵌入脚本

```bash
bash /home/william/Rux/kernel/embed_user_programs.sh
```

**输出**：
```
正在嵌入用户程序: /home/william/Rux/userspace/target/riscv64gc-unknown-none-elf/release/hello_world
✓ 用户程序已嵌入到: /home/william/Rux/kernel/src/embedded_user_programs.rs
  大小: 5192 字节
```

### 嵌入的数据结构

**文件**：`kernel/src/embedded_user_programs.rs`

```rust
//! 嵌入的用户程序
//!
/// 这个文件由 embed_user_programs.sh 自动生成

/// 嵌入的 hello_world 用户程序 (ELF 格式)
pub static HELLO_WORLD_ELF: &[u8] = &[
    0x7f, 0x45, 0x4c, 0x46,  // ELF header
    0x02, 0x01, 0x01, 0x00,
    // ... 更多字节 ...
];
```

---

## 系统调用

### 系统调用约定

**寄存器约定**（RISC-V Linux ABI）：
- `a7`: 系统调用号
- `a0-a5`: 参数（最多 6 个）
- `a0`: 返回值

**系统调用号**：

| 系统调用号 | 名称 | 参数 | 说明 |
|-----------|------|------|------|
| 63 | sys_read | fd, buf, count | 读文件 |
| 64 | sys_write | fd, buf, count | 写文件 |
| 56 | sys_openat | dfd, filename, flags, mode | 打开文件 |
| 57 | sys_close | fd | 关闭文件 |
| 93 | sys_exit | error_code | 退出进程 |
| 172 | sys_getpid | - | 获取进程 ID |
| 110 | sys_getppid | - | 获取父进程 ID |

### 系统调用示例

#### 1. 简单退出

```rust
use core::arch::asm;

pub unsafe fn syscall1(n: u64, a0: u64) -> u64 {
    let mut ret: u64;
    asm!(
        "ecall",
        inlateout("a7") n => _,
        inlateout("a0") a0 => ret,
        options(nostack, nomem)
    );
    ret
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 调用 sys_exit(0)
    syscall1(93, 0);

    loop { asm!("nop", options(nomem, nostack)); }
}
```

#### 2. 多个系统调用

```rust
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 获取进程 ID
    let pid = syscall1(172, 0);

    // 获取父进程 ID
    let ppid = syscall1(110, 0);

    // 退出程序
    syscall1(93, 0);

    loop { asm!("nop", options(nomem, nostack)); }
}
```

#### 3. 带参数的系统调用

```rust
pub unsafe fn syscall3(n: u64, a0: u64, a1: u64, a2: u64) -> u64 {
    let mut ret: u64;
    asm!(
        "ecall",
        inlateout("a7") n => _,
        inlateout("a0") a0 => ret,
        inlateout("a1") a1 => _,
        inlateout("a2") a2 => _,
        options(nostack, nomem)
    );
    ret
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 写字符串到标准输出
    let msg = b"Hello from user mode!\n";
    syscall3(64, 1, msg.as_ptr() as u64, msg.len() as u64);

    // 退出
    syscall1(93, 0);

    loop { asm!("nop", options(nomem, nostack)); }
}
```

---

## 调试技巧

### 1. 添加调试输出

由于 no_std 环境，不能使用 `println!`。使用内联汇编直接写入 UART：

```rust
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 调试：写入字符到 UART
    unsafe {
        const UART: u64 = 0x10000000;

        // 写入 'H'
        core::ptr::write_volatile(UART as *mut u8, b'H');
        // 写入 'i'
        core::ptr::write_volatile(UART as *mut u8, b'i');
        // 写入换行
        core::ptr::write_volatile(UART as *mut u8, b'\n');
    }

    // 继续正常执行
    syscall1(93, 0);

    loop { asm!("nop", options(nomem, nostack)); }
}
```

### 2. 查看系统调用返回值

```rust
#[no_mangle]
pub extern "C" fn _start() -> ! {
    let pid = syscall1(172, 0);

    // 根据 pid 值采取不同行动
    if pid == 0 {
        // 没有进程 ID，可能出错
        loop { asm!("nop", options(nomem, nostack)); }
    }

    syscall1(93, pid);  // 使用 pid 作为退出码

    loop { asm!("nop", options(nomem, nostack)); }
}
```

### 3. 使用无限循环标记位置

```rust
#[no_mangle]
pub extern "C" fn _start() -> ! {
    syscall1(93, 0);

    // 如果 sys_exit 失败，会到达这里
    loop {
        unsafe { asm!("wfi", options(nomem, nostack)) };
    }
}
```

---

## 已知限制

### 当前限制

1. **没有文件系统** - sys_openat/sys_read/sys_write 部分实现
2. **没有进程管理** - sys_getpid 返回 0（没有当前进程）
3. **嵌入式加载** - 用户程序嵌入在内核中（不是从文件系统加载）
4. **单页表** - 使用单一页表，不支持进程地址空间隔离

### 未来计划

1. **文件系统** - 实现完整的 VFS 和 ext4 支持
2. **进程管理** - 实现进程控制块（PCB）和调度
3. **ELF 加载器** - 从文件系统加载用户程序
4. **execve 系统调用** - 替换当前进程映像

---

## 参考资料

- [RISC-V Linux ABI](https://github.com/riscv-non-isa/riscv-elf-psabi-doc)
- [RISC-V 特权级架构规范](https://riscv.org/specifications/privileged-isa/)
- [ELF 格式规范](https://refspecs.linuxfoundation.org/elf/elf.pdf)
- [Linux 系统调用表](https://github.com/torvalds/linux/blob/master/arch/riscv/include/asm/unistd.h)
