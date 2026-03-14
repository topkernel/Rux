# Rux

<div align="center">

**A Linux-like OS kernel entirely written in Rust**

[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-riscv64-informational.svg)](https://github.com/rust-osdev/rust-embedded)
[![Tests](https://img.shields.io/badge/tests-1%2C913%20cases-brightgreen.svg)](docs/guides/testing.md)
[![Code](https://img.shields.io/badge/code-63%2C200%20lines-blue.svg)](docs/architecture/structure.md)

**Default Platform: RISC-V 64-bit (RV64GC)**

</div>

---

## 🤖 AI Generation Statement

**This project's code is developed with AI assistance (Claude Code + GLM5).**

- Uses Anthropic Claude Code CLI tool for assisted development
- Follows POSIX standards and maintains 100% Linux ABI compatibility
- Aims to explore the possibilities and limitations of **AI-assisted OS kernel development**

---

## 🎯 Project Goals

### ⚠️ Core Principle: Full POSIX/ABI Compatibility

**Core Objective**: A Linux-compatible OS kernel written in Rust

- ✅ **100% POSIX Compatible** - Full compliance with POSIX standards
- ✅ **Linux ABI Compatible** - Can run native Linux userspace programs directly
- ✅ **System Call Compatible** - Uses Linux system call numbers and interfaces
- ✅ **Filesystem Compatible** - Supports ext4 and other Linux filesystems
- ✅ **ELF Format Compatible** - Executable format identical to Linux

**Design Philosophy**:
- External interfaces must be 100% compatible with Linux
- Internal implementation can use better designs when it doesn't affect compatibility
- Welcoming improvements that maintain Linux ecosystem compatibility

---

## 📊 Project Status

| Metric | Value | Details |
|--------|-------|---------|
| **Lines of Code** | ~63,200 lines | [Code Structure](docs/architecture/structure.md) |
| **Source Files** | 196 Rust files | [Project Structure](docs/architecture/structure.md) |
| **Kernel Tests** | 51 test files | [Testing Guide](docs/guides/testing.md) |
| **mini-ltp** | 24 compatibility tests | [Testing Guide](docs/guides/testing.md) |
| **Linux LTP** | 1,838 official tests | [Testing Guide](docs/guides/testing.md) |
| **Platform Support** | RISC-V 64-bit | [Roadmap](docs/progress/roadmap.md) |

**Module Distribution**:
- Filesystem (fs/): 15,334 lines (24.3%)
- Device Drivers (drivers/): 8,005 lines (12.7%)
- Unit Tests (tests/): 7,376 lines (11.7%)
- System Calls (syscall/): 5,654 lines (9.0%)
- Network Stack (net/): 5,177 lines (8.2%)
- Architecture (arch/): 5,168 lines (8.2%)
- Memory Management (mm/): 4,268 lines (6.8%)
- Process Scheduling (sched/): 4,255 lines (6.7%)
- Process Management (process/): 2,549 lines (4.0%)
- Sync Primitives (sync/): 1,147 lines (1.8%)

---

## 🚀 Quick Start

### Prerequisites

```bash
# Rust toolchain (nightly recommended)
rustc --version
cargo --version

# QEMU system emulator
qemu-system-riscv64 --version

# RISC-V target
rustup target add riscv64gc-unknown-none-elf
```

### Build and Run

```bash
# Build kernel
make build

# Build userspace programs (shell, apps, mini-ltp, toybox)
make user

# Build Rootfs image
make rootfs

# Run kernel (default shell)
make run

# Run GUI desktop
make gui

# Run unit tests
make test
```

For detailed instructions: [Getting Started Guide](docs/guides/getting-started.md)

---

## 🏆 Shell Boot Log

```
██████  ██    ██ ██   ██
██   ██ ██    ██  ██ ██
██████  ██    ██   ███
██   ██ ██    ██  ██ ██
██   ██  ██████  ██   ██
  [ RISC-V 64-bit | POSIX Compatible | v0.1.0 ]

Kernel starting...

Module            Description                        Status
----------------  --------------------------------   --------
console:          UART ns16550a driver               [ok]
smp:              4 CPU(s) online                    [ok]
trap:             stvec handler installed            [ok]
trap:             ecall syscall handler              [ok]
mm:               Sv39 3-level page table            [ok]
mm:               satp CSR configured                [ok]
mm:               buddy allocator order 0-12         [ok]
mm:               heap region 32MB @ 0x80A00000      [ok]
mm:               slab allocator 4MB                 [ok]
boot:             FDT/DTB parsed                     [ok]
boot:             cmd: root=/dev/vda rw init=...     [ok]
mm:               user frame allocator 64MB          [ok]
mm:               32768 page descriptors             [ok]
intc:             PLIC @ 0x0C000000                  [ok]
intc:             external IRQ routing               [ok]
ipi:              SSIP software IRQ                  [ok]
bio:              buffer cache layer                 [ok]
fs:               ext4 driver loaded                 [ok]
fs:               ramfs mounted /                    [ok]
fs:               procfs initialized                 [ok]
fs:               procfs mounted /proc               [ok]
driver:           virtio-blk PCI x1                  [ok]
driver:           GenDisk registered                 [ok]
fs:               ext4 mounted /                     [ok]
fs:               procfs remounted /proc             [ok]
driver:           virtio-net x1                      [ok]
sched:            CFS scheduler v1                   [ok]
sched:            runqueue per-CPU                   [ok]
sched:            PID allocator init                 [ok]
sched:            idle task (PID 0)                  [ok]
mm:               PCP cpu2 hotpage                   [ok]
trap:             sie.SEIE enabled                   [ok]
driver:           virtio-gpu probed                  [ok]
gpu:              1280x800 32bpp framebuffer         [ok]
fs:               devfs mounted /dev                 [ok]
driver:           evdev /dev/input/event0            [ok]
driver:           evdev /dev/input/event1            [ok]
driver:           PS/2 keyboard (stub)               [ok]
driver:           PS/2 mouse (stub)                  [ok]

init:             loading /bin/shell                 [ok]
init:             ELF loaded to user space           [ok]
init:             init task (PID 1) enqueued         [ok]


========================================
  Rux OS Shell v0.4 (musl libc)
========================================
Type 'help' for available commands

rux>
```

## GUI Boot
<img width="1362" height="1070" alt="image" src="https://github.com/user-attachments/assets/a485db2a-ab4e-4123-a67e-24fbf5d43752" />

---

## 📁 Project Structure

```
Rux/
├── kernel/                 # Kernel source (~63,200 lines)
│   ├── src/
│   │   ├── fs/           # Filesystem (15,334 lines)
│   │   │   ├── ext4/     # ext4 filesystem
│   │   │   ├── devfs/    # devfs device filesystem
│   │   │   └── procfs.rs # procfs process filesystem
│   │   ├── drivers/      # Device drivers (8,005 lines)
│   │   │   ├── gpu/      # GPU/framebuffer drivers
│   │   │   ├── input/    # Input device drivers
│   │   │   ├── virtio/   # VirtIO devices
│   │   │   └── net/      # Network devices
│   │   ├── tests/        # Unit tests (51 files)
│   │   ├── syscall/      # System calls (5,654 lines)
│   │   ├── net/          # Network stack (5,177 lines)
│   │   ├── arch/         # RISC-V architecture (5,168 lines)
│   │   ├── mm/           # Memory management (4,268 lines)
│   │   ├── sched/        # Process scheduling (4,255 lines)
│   │   ├── process/      # Process management (2,549 lines)
│   │   └── sync/         # Sync primitives (1,147 lines)
│   └── build.rs          # Build script
├── userspace/            # Userspace programs
│   ├── shell/            # Default shell (no_std Rust)
│   ├── apps/             # GUI apps (desktop, calculator, clock, vshell)
│   ├── libs/gui/         # GUI library (rux_gui)
│   ├── tests/mini-ltp/   # Kernel compatibility tests (24)
│   ├── linux-ltp/        # Official LTP tests (1,838)
│   └── toybox/           # Toybox (BusyBox alternative)
├── toolchain/            # Toolchain (musl libc)
├── docs/                 # 📚 Documentation center
├── test/                 # Test scripts
└── Cargo.toml            # Workspace configuration
```

Detailed structure: [Project Structure Documentation](docs/architecture/structure.md)

---

## ✨ Key Features

### Implemented Features

- **Process Management**: fork/execve/wait4/signal handling/CFS scheduler
- **Memory Management**: Sv39 page table/Buddy allocator/Slab allocator/COW
- **Filesystem**: ext4/procfs/devfs/ramfs
- **Device Drivers**: VirtIO-blk/net/gpu/input, framebuffer, evdev
- **Network Stack**: TCP/UDP/IPv4/ARP/Socket API
- **SMP Multi-core**: 4-core support/load balancing/IPI
- **GUI**: Desktop environment/calculator/clock/visual shell

### System Calls

Supports 80+ Linux system calls, including:
- File: openat/close/read/write/lseek/fstat/mkdir/unlink
- Process: fork/execve/wait4/exit/getpid/getppid/kill
- Memory: brk/mmap/munmap/mprotect
- Signal: kill/sigaction/sigprocmask
- Network: socket/bind/listen/accept/connect/sendto/recvfrom
- IPC: pipe/pipe2/select/poll/eventfd

---

## 📚 Documentation

### Core Documentation

- **[Getting Started](docs/guides/getting-started.md)** - Up and running in 5 minutes
- **[Roadmap](docs/progress/roadmap.md)** - Phase planning and current status (Phase 24)
- **[Project Structure](docs/architecture/structure.md)** - Source code organization
- **[Design Principles](docs/architecture/design.md)** - POSIX compatibility and Linux ABI alignment

### Architecture Documentation

- **[RISC-V Architecture](docs/architecture/riscv64.md)** - RV64GC support details
- **[Boot Process](docs/architecture/boot.md)** - From OpenSBI to kernel boot
- **[Changelog](docs/development/changelog.md)** - Version history and update records

### Development Guides

- **[Development Workflow](docs/guides/development.md)** - Contributing code and development standards
- **[User Programs](docs/development/user-programs.md)** - ELF loading and execve

---

## 🧪 Test Status

### Kernel Unit Tests
- **Test Files**: 51
- **Coverage**: Memory, process, filesystem, network, drivers, etc.

### mini-ltp Kernel Compatibility Tests
- **Test Count**: 24
- **Coverage**: Core system calls like fork, fileio, pipe, mmap, signal, execve

### Linux LTP Test Suite
- **Test Count**: 1,838
- **LTP Version**: 20240524
- **Compile Rate**: 101% (musl libc cross-compilation)
- **Coverage**: Syscalls (1,378), memory (108), containers (46), filesystem (29), security (24), scheduler (23), IO (19)

---

## 🤝 Contributing

Contributions are welcome! Please check the [Roadmap](docs/progress/roadmap.md) for tasks that need help.

### Development Standards

- Follow [Conventional Commits](https://www.conventionalcommits.org/) specification
- Refer to [Development Workflow](docs/guides/development.md) for development standards

**Core Principles**:
- ✅ Strictly follow POSIX standards and Linux ABI
- ✅ External interfaces must be 100% compatible with Linux
- ✅ Internal implementation can use any design approach
- ✅ Welcoming any improvements that maintain compatibility

---

## 📄 License

MIT License - See [LICENSE](LICENSE) for details

---

## 🙏 Acknowledgments

This project is inspired by:

- [Linux Kernel](https://www.kernel.org/)

---

<div align="center">

**Made with ❤️ and Rust + AI**

[Project Home](https://github.com/topkernel/rux) • [Issue Tracker](https://github.com/topkernel/rux/issues)

</div>
