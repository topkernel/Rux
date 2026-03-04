# 用户程序开发指南

本文档说明如何在 Rux OS 中开发和运行用户程序。

**最后更新**：2026-03-04
**状态**：✅ Shell、Toybox、GUI 应用完全可用

---

## 目录

- [概述](#概述)
- [用户程序类型](#用户程序类型)
- [no_std 用户程序](#no_std-用户程序)
- [musl libc 程序](#musl-libc-程序)
- [系统调用](#系统调用)
- [调试技巧](#调试技巧)

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
```

---

## 用户程序类型

Rux OS 支持多种类型的用户程序：

| 类型 | 状态 | 描述 |
|------|------|------|
| **no_std Rust** | ✅ 完全可用 | 裸机 Rust 程序，无标准库 |
| **musl libc C** | ✅ 完全可用 | C 程序，Toybox 验证通过 |
| **GUI 应用** | ✅ 完全可用 | 桌面环境、计算器、时钟 |

### 当前可用的用户程序

| 程序 | 类型 | 说明 |
|------|------|------|
| `/bin/shell` | no_std Rust | 默认 Shell |
| `/bin/toybox` | musl libc | BusyBox 替代品 |
| `/app/desktop` | musl libc + GUI | 桌面环境 |
| `/app/calculator` | musl libc + GUI | 计算器 |
| `/app/clock` | musl libc + GUI | 时钟 |
| `/app/vshell` | musl libc + GUI | 可视化 Shell |

---

## no_std 用户程序

### 最小示例

```rust
#![no_std]
#![no_main]

use core::panic::PanicInfo;

// 系统调用号（RISC-V Linux ABI）
const SYS_EXIT: u64 = 93;

// 系统调用函数
pub unsafe fn syscall1(n: u64, a0: u64) -> u64 {
    let mut ret: u64;
    core::arch::asm!(
        "ecall",
        inlateout("a7") n => _,
        inlateout("a0") a0 => ret,
        lateout("a1") _,
        options(nostack, nomem)
    );
    ret
}

// 程序入口点
#[no_mangle]
pub extern "C" fn _start() -> ! {
    // 调用 sys_exit(0)
    syscall1(SYS_EXIT, 0);

    loop {
        unsafe { core::arch::asm!("nop", options(nomem, nostack)); }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {
        unsafe { core::arch::asm!("nop", options(nomem, nostack)); }
    }
}
```

---

## musl libc 程序

### 构建工具链

```bash
cd toolchain
bash build-musl.sh
```

### C 程序示例

```c
#include <unistd.h>
#include <stdio.h>

int main(int argc, char *argv[]) {
    printf("Hello from Rux OS!\n");
    return 0;
}
```

### 编译

```bash
riscv64-linux-gnu-gcc -static -o hello hello.c
```

### musl 链接器脚本

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

### 已实现的系统调用 (80+)

**文件操作**：

| 系统调用号 | 名称 | 说明 |
|-----------|------|------|
| 56 | sys_openat | 打开文件 |
| 57 | sys_close | 关闭文件 |
| 63 | sys_read | 读文件 |
| 64 | sys_write | 写文件 |
| 62 | sys_lseek | 定位文件 |
| 80 | sys_fstat | 获取文件状态 |

**进程操作**：

| 系统调用号 | 名称 | 说明 |
|-----------|------|------|
| 93 | sys_exit | 退出进程 |
| 172 | sys_getpid | 获取进程 ID |
| 110 | sys_getppid | 获取父进程 ID |
| 220 | sys_clone | 创建进程/线程 |
| 221 | sys_execve | 执行程序 |
| 260 | sys_wait4 | 等待子进程 |

**内存操作**：

| 系统调用号 | 名称 | 说明 |
|-----------|------|------|
| 214 | sys_brk | 调整堆 |
| 222 | sys_mmap | 内存映射 |
| 215 | sys_munmap | 取消映射 |
| 226 | sys_mprotect | 修改保护 |

**网络操作**：

| 系统调用号 | 名称 | 说明 |
|-----------|------|------|
| 198 | sys_socket | 创建套接字 |
| 200 | sys_bind | 绑定地址 |
| 201 | sys_listen | 监听连接 |
| 202 | sys_accept | 接受连接 |
| 203 | sys_connect | 发起连接 |

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

### 2. 使用 GDB 调试

```bash
# 启动 QEMU 带 GDB 支持
qemu-system-riscv64 -M virt -nographic -kernel rux.elf -s -S

# 另一个终端启动 GDB
riscv64-unknown-elf-gdb
(gdb) target remote :1234
(gdb) break _start
(gdb) continue
```

### 3. mini-ltp 测试

```bash
# 在 Rux 中运行
cd /test/mini-ltp
./run_tests.sh
```

---

## rootfs 目录结构

```
/
├── bin/                # 基本命令
│   ├── shell           # Shell
│   ├── sh -> shell     # Shell 符号链接
│   ├── toybox          # Toybox
│   ├── ls -> toybox    # 常用命令符号链接
│   └── cat -> toybox
│
├── app/                # GUI 应用
│   ├── desktop         # 桌面环境
│   ├── calculator      # 计算器
│   ├── clock           # 时钟
│   └── vshell          # 可视化 Shell
│
├── test/               # 测试程序
│   └── mini-ltp/       # 内核兼容性测试
│
├── dev/                # 设备文件
├── proc/               # procfs 挂载点
└── tmp/                # 临时文件
```

---

## 参考资料

- [RISC-V Linux ABI](https://github.com/riscv-non-isa/riscv-elf-psabi-doc)
- [RISC-V 特权级架构规范](https://riscv.org/specifications/privileged-isa/)
- [ELF 格式规范](https://refspecs.linuxfoundation.org/elf/elf.pdf)
- [Linux 系统调用表](https://github.com/torvalds/linux/blob/master/arch/riscv/include/asm/unistd.h)

---

**文档版本**：v2.0.0
**最后更新**：2026-03-04
