# Rux 用户程序构建系统

## 概述

这个目录包含 Rux 内核的用户空间程序。所有程序编译为独立的二进制文件，可以通过内核加载执行。

## 目录结构

```
userspace/
├── Cargo.toml              # Rust 工作空间配置
├── .cargo/
│   └── config.toml         # Cargo 配置
├── build.sh                # 构建脚本
├── Makefile                # 构建自动化
├── README.md               # 本文件
│
├── shell/                  # Shell 程序 (C + musl libc)
│   ├── Makefile
│   ├── shell.ld            # 链接脚本
│   └── src/
│       └── shell.c
│
├── desktop/                # 桌面环境 (Rust std)
│   ├── Cargo.toml
│   └── src/
│       └── main.rs
│
├── libs/                   # 库文件
│   └── gui/                # GUI 库 (Rust std)
│       ├── Cargo.toml
│       └── src/
│
└── toybox/                 # Toybox (200+ Linux 命令行工具)
    └── build-toybox.sh
```

## 开发环境

### 前置要求

- Rust 工具链（stable）
- RISC-V GCC 工具链：`riscv64-linux-gnu-gcc`（用于编译 shell）
- musl libc 工具链（在 `toolchain/riscv64-rux-linux-musl/`）

### 本地开发（x86_64）

desktop 和 rux_gui 使用标准库（std），可以在本地进行开发和测试：

```bash
cd userspace

# 构建所有程序
./build.sh

# 构建 release 版本
./build.sh release

# 清理构建产物
./build.sh clean
```

### 交叉编译到 RISC-V

如需交叉编译到 RISC-V 目标：

```bash
# 安装 RISC-V 目标
rustup target add riscv64gc-unknown-linux-gnu

# 交叉编译
cargo build --target riscv64gc-unknown-linux-gnu
```

## 用户程序

### shell

命令行 Shell，使用 C 语言和 musl libc 构建。

**位置**：`shell/`

**特点**：
- 交互式命令行
- 命令执行和管道
- 使用自定义链接脚本（shell.ld）

**构建**：
```bash
make -C shell
# 或从项目根目录
make shell
```

### desktop

桌面环境，使用 Rust std 构建。

**位置**：`desktop/`

**依赖**：`libs/gui`

**特点**：
- 使用标准库
- 可在本地开发测试
- 条件编译支持 RISC-V 系统调用

**构建**：
```bash
./build.sh release
```

### rux_gui

GUI 库，使用 Rust std 构建。

**位置**：`libs/gui/`

**功能**：
- 基础绘图原语
- 字体渲染
- 双缓冲
- 窗口管理
- UI 控件

**平台支持**：
- RISC-V：使用内联汇编进行系统调用
- 其他平台：返回 stub 值（用于开发测试）

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

## 构建命令

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

# 运行内核
make run
```

## 系统调用接口

在 RISC-V 目标上，程序使用 Linux ABI 系统调用：

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

### 检查二进制文件

```bash
# 查看文件信息
file shell/shell

# 查看程序大小
ls -lh shell/shell

# 使用 readelf 查看 RISC-V ELF
riscv64-linux-gnu-readelf -h shell/shell
```

### 本地运行

```bash
# desktop 可以在本地运行（但 framebuffer 会失败）
./target/debug/desktop
```
