# Rux 用户程序构建系统

## 概述

这个目录包含 Rux 内核的用户空间程序。所有程序编译为独立的 ELF 二进制文件，可以通过内核的 ELF 加载器执行。

## 目录结构

```
userspace/
├── Cargo.toml              # Rust 工作空间配置
├── .cargo/
│   └── config.toml         # RISC-V 交叉编译配置
├── build.sh                # 构建脚本
├── Makefile                # 构建自动化
├── README.md               # 本文件
│
├── shell/                  # Shell 程序 (C + musl libc)
│   ├── Makefile
│   ├── shell.ld            # 链接脚本
│   ├── src/
│   │   └── shell.c
│   └── shell               # 编译产物
│
├── desktop/                # 桌面环境 (Rust no_std)
│   ├── Cargo.toml
│   ├── user.ld             # 链接脚本
│   └── src/
│       └── main.rs
│
├── libs/                   # 库文件
│   └── gui/                # GUI 库 (Rust no_std)
│       ├── Cargo.toml
│       └── src/
│
└── toybox/                 # Toybox (200+ Linux 命令行工具)
    └── build-toybox.sh
```

## 快速开始

### 前置要求

- Rust 工具链（stable）
- RISC-V target：`riscv64gc-unknown-none-elf`
- RISC-V GCC 工具链：`riscv64-linux-gnu-gcc`
- musl libc 工具链（在 `toolchain/riscv64-rux-linux-musl/`）

### 安装 RISC-V 目标

```bash
rustup target add riscv64gc-unknown-none-elf
```

### 构建命令

从项目根目录执行：

```bash
# 构建所有用户程序（shell, desktop 等）
make user

# 单独构建 shell
make shell

# 构建 toybox
make toybox

# 创建 rootfs 镜像
make rootfs
```

或直接在 userspace 目录执行：

```bash
# 构建所有程序 (debug)
./build.sh

# 构建所有程序 (release)
./build.sh release

# 清理构建产物
./build.sh clean
```

## 用户程序

### shell

命令行 Shell，使用 C 语言和 musl libc 构建。

**位置**：`shell/`

**功能**：
- 交互式命令行
- 命令执行
- 管道和重定向

**构建**：
```bash
make -C shell
```

### desktop

桌面环境，使用 Rust no_std 构建。

**位置**：`desktop/`

**依赖**：`libs/gui`

**构建**：
```bash
./build.sh release
```

### toybox

200+ Linux 命令行工具的集合。

**位置**：`toybox/`

**包含命令**：
- 文件操作：ls, cat, cp, mv, rm, mkdir, ln, touch
- 文本处理：echo, head, tail, wc, sort, uniq, grep, sed, awk
- 系统信息：uname, hostname, id, whoami, free, df, du
- 其他：date, sleep, true, false, test, env, yes, tee

**构建**：
```bash
make toybox
```

## 链接脚本

每个可执行程序有自己的链接脚本：

| 程序 | 链接脚本 | 说明 |
|------|----------|------|
| shell | `shell/shell.ld` | C + musl，单内存区域 |
| desktop | `desktop/user.ld` | Rust no_std，单内存区域 |

两者都使用相同的内存布局：
- 起始地址：0x10000
- 大小：1MB
- 包含代码段、数据段、栈、堆

## 系统调用接口

用户程序使用 RISC-V Linux ABI 的系统调用约定：

### 寄存器约定

- **a7**: 系统调用号
- **a0-a5**: 参数（最多 6 个）
- **a0**: 返回值

### 常用系统调用

| 系统调用 | 号码 | 功能 |
|----------|------|------|
| read | 63 | 读取文件 |
| write | 64 | 写入文件 |
| exit | 93 | 退出程序 |
| getpid | 172 | 获取进程 ID |

## 调试

### 检查 ELF 文件

```bash
# 查看文件信息
file shell/shell

# 查看程序大小
ls -lh shell/shell

# 使用 readelf 查看详细信息
riscv64-linux-gnu-readelf -h shell/shell
```

## 运行

```bash
# 运行内核（默认使用 shell）
make run

# 运行图形界面模式
make gui
```
