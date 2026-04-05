# Rux

<div align="center">

**A Linux-like OS kernel entirely written in Rust**

[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-riscv64-informational.svg)](https://github.com/rust-osdev/rust-embedded)
[![Tests](https://img.shields.io/badge/tests-1%2C939%20cases-brightgreen.svg)](docs/test/testing.md)
[![Code](https://img.shields.io/badge/code-95%2C000%20lines-blue.svg)](docs/architecture/structure.md)

**Default Platform: RISC-V 64-bit (RV64GC)**

</div>

---

## 🤖 AI Generation Statement

**This project's code is developed with AI assistance (Claude Code + Opus4.6/GLM5.1/Minimax2.7).**

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
| **Lines of Code** | ~95,000 lines | [Code Structure](docs/architecture/structure.md) |
| **Source Files** | 260 files (256 Rust + 3 ASM + 1 LD) | [Project Structure](docs/architecture/structure.md) |
| **Kernel Tests** | 59 test files | [Testing Guide](docs/test/testing.md) |
| **Smoke Tests** | 17 tests (all passing) | [Testing Guide](docs/test/testing.md) |
| **mini-ltp** | 25 compatibility tests | [Testing Guide](docs/test/testing.md) |
| **Linux LTP** | 1,838 official tests | [Testing Guide](docs/test/testing.md) |
| **Platform Support** | RISC-V 64-bit | [Roadmap](docs/progress/roadmap.md) |

**Module Distribution**:
- Filesystem (fs/): 22,012 lines (23.2%)
- System Calls (syscall/): 11,573 lines (12.2%)
- Unit Tests (tests/): ~9,600 lines (10.1%)
- Device Drivers (drivers/): 8,913 lines (9.4%)
- Architecture (arch/): 9,079 lines (9.6%)
- Memory Management (mm/): 7,656 lines (8.1%)
- Top-level: 6,030 lines (6.4%)
- Network Stack (net/): 5,319 lines (5.6%)
- Process Management (process/): 4,312 lines (4.5%)
- Process Scheduling (sched/): 3,467 lines (3.7%)
- IPC (ipc/): 2,576 lines (2.7%)
- Sync Primitives (sync/): 1,955 lines (2.1%)
- Interrupt (interrupt/): 1,649 lines (1.7%)
- Diagnostics (dfx/): 1,027 lines (1.1%)

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
trap:             stvec handler installed            [ok]
trap:             ecall syscall handler              [ok]
mm:               Sv39 3-level page table            [ok]
mm:               satp CSR configured                [ok]
mm:               buddy allocator order 0-12         [ok]
mm:               heap region 32MB @ 0x80A00000      [ok]
mm:               slab allocator 4MB                 [ok]
boot:             FDT/DTB parsed                     [ok]
boot:             cmd: root=/dev/vda rw init=...     [ok]
mm:               linear mapping 2048 MB             [ok]
mm:               vmemmap mapping initialized        [ok]
mm:               layout: kernel=0x80200000-0x80a0   [ok]
mm:               layout: heap=0x80a00000-0x82a000   [ok]
mm:               524288 page descriptors            [ok]
mm:               zone allocator initialized         [ok]
memblock:         total 2048MB, available 2038MB     [ok]
mm:               device mappings created            [ok]
irq:              irq_desc array initialized         [ok]
intc:             PLIC @ 0x0C000000                  [ok]
intc:             IRQ domain + chip registered       [ok]
ipi:              SSIP software IRQ + bitmap multi   [ok]
console:          UART interrupt-driven RX           [ok]
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
softirq:          ksoftirqd per-CPU threads          [ok]
dfx:              diagnostic subsystem               [ok]
smp:              4 CPUs online                      [ok]
trap:             sie.SEIE enabled                   [ok]
fs:               devfs mounted /dev                 [ok]
driver:           evdev /dev/input/event0            [ok]
driver:           evdev /dev/input/event1            [ok]
driver:           PS/2 keyboard (stub)               [ok]
driver:           PS/2 mouse (stub)                  [ok]
init:             loading /bin/sh                    [ok]
init:             ELF loaded to user space           [ok]
init:             init task (PID 1) enqueued         [ok]

Welcome to Rux OS (RISC-V 64)
- mrsh (POSIX shell) | A minimal POSIX-compatible shell
root:/#
```

## GUI Boot
<img width="1362" height="1070" alt="image" src="https://github.com/user-attachments/assets/a485db2a-ab4e-4123-a67e-24fbf5d43752" />

---

## 📁 Project Structure

```
Rux/
├── kernel/                 # Kernel source (~95,000 lines)
│   ├── src/
│   │   ├── fs/           # Filesystem (22,012 lines)
│   │   │   ├── ext4/     # ext4 filesystem
│   │   │   ├── jbd2/     # JBD2 journaling layer
│   │   │   ├── devfs/    # devfs device filesystem
│   │   │   └── procfs/   # procfs process filesystem
│   │   ├── arch/         # RISC-V architecture (9,079 lines)
│   │   │   ├── mm/       # Arch-specific MM (pt, fixmap, ASID, page fault)
│   │   │   ├── boot.S    # MMU trampoline, VMA/LMA linking
│   │   │   ├── trap.S    # PtRegs save/restore, ret_from_fork
│   │   │   └── uaccess.S # User memory access assembly
│   │   ├── drivers/      # Device drivers (8,913 lines)
│   │   │   ├── gpu/      # GPU/framebuffer drivers
│   │   │   ├── input/    # Input device drivers
│   │   │   ├── virtio/   # VirtIO devices (blk/net/gpu/input)
│   │   │   └── net/      # Network devices
│   │   ├── mm/           # Memory management (7,656 lines)
│   │   │   ├── Zone allocator (DMA/DMA32/NORMAL/MOVABLE)
│   │   │   ├── vmemmap, buddy, slab, PCP, memblock
│   │   │   ├── VMA, mm_struct, page fault, COW
│   │   │   └── rmap, hugepage, meminfo
│   │   ├── tests/        # Unit tests (59 files)
│   │   ├── syscall/      # System calls (11,573 lines, 340+ syscalls)
│   │   ├── ipc/          # IPC (2,576 lines) — System V, POSIX MQ
│   │   ├── net/          # Network stack (5,319 lines)
│   │   ├── sched/        # Process scheduling (3,467 lines)
│   │   │   ├── CFS, RT (FIFO/RR), Deadline (EDF+CBS), Idle
│   │   ├── process/      # Process management (4,312 lines)
│   │   ├── sync/         # Sync primitives (1,955 lines)
│   │   ├── interrupt/    # Interrupt subsystem (1,649 lines)
│   │   └── dfx/          # Diagnostics/DFX (1,027 lines)
│   └── build.rs          # Build script
├── userspace/            # Userspace programs
│   ├── shell/            # Default shell (musl libc)
│   ├── apps/             # GUI apps (desktop, calculator, clock, vshell)
│   ├── libs/gui/         # GUI library (rux_gui)
│   ├── tests/mini-ltp/   # Kernel compatibility tests (25)
│   ├── tests/smoke_test/ # Smoke tests (17 tests, all passing)
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

- **Process Management**: fork/execve/wait4/signal handling/CFS scheduler/clone flags/gettid
- **Memory Management**: Sv39 page table/Zone allocator/vmemmap/PCP/COW/Demand paging/ASID/MAP_PRIVATE COW
- **Filesystem**: ext4/procfs/devfs/ramfs/JBD2 journaling
- **IPC**: System V semaphores/message queues/shared memory, POSIX message queues
- **Device Drivers**: VirtIO-blk/net/gpu/input, framebuffer, evdev
- **Network Stack**: TCP/UDP/IPv4/ARP/Socket API
- **SMP Multi-core**: 4-core support/load balancing/IPI/per-CPU idle tasks
- **Linux-Style Boot**: MMU trampoline/VMA-LMA linking/PtRegs at stack top
- **GUI**: Desktop environment/calculator/clock/visual shell

### System Calls

Supports 340+ Linux system calls (~88% coverage), including:
- File: openat/close/read/write/readv/writev/pread64/pwrite64/lseek/fstat/getdents64/mkdirat/rmdir/unlinkat/sendfile/statfs/copy_file_range/statx
- Process: fork/execve/wait4/exit/getpid/getppid/gettid/kill/clone/sched_yield/prctl/getrusage
- Memory: brk (expand+shrink)/mmap/munmap (MAP_PRIVATE COW)/mprotect/mremap/madvise/msync
- Signal: sigaction/sigprocmask/sigreturn/sigaltstack/sigpending/sigtimedwait
- Network: socket/bind/listen/accept/connect/sendto/recvfrom/sendmsg/recvmsg
- IPC: pipe/pipe2/dup/dup3/select/poll/epoll/eventfd/futex/shmget/shmat/shmdt/msgget/msgsnd/msgrcv/semget/semop/mq_open/mq_send/mq_receive

---

## 📚 Documentation

### Core Documentation

- **[Getting Started](docs/guides/getting-started.md)** - Up and running in 5 minutes
- **[Roadmap](docs/progress/roadmap.md)** - Phase planning and current status (Phase 37)
- **[Project Structure](docs/architecture/structure.md)** - Source code organization
- **[Design Principles](docs/architecture/design.md)** - POSIX compatibility and Linux ABI alignment

### Architecture Documentation

- **[RISC-V Architecture](docs/architecture/riscv64.md)** - RV64GC support details
- **[Boot Process](docs/architecture/boot.md)** - MMU trampoline, VMA/LMA linking, page table init
- **[Memory Management](docs/architecture/memory.md)** - Zone allocator, vmemmap, COW, demand paging
- **[Changelog](docs/progress/changelog.md)** - Version history and update records

### Development Guides

- **[Development Workflow](docs/guides/development.md)** - Contributing code and development standards
- **[Boot Process](docs/architecture/boot.md)** - From OpenSBI to kernel boot

---

## 🧪 Test Status

### Smoke Tests
- **Test Count**: 17 (all passing)
- **Coverage**: File I/O, process management, memory, signals, O_CLOEXEC, sendfile, wait4, process groups, setsid, credentials, readv/writev, gettid, pwrite64, dup3, kill, statfs, sched_yield

### Kernel Unit Tests
- **Test Files**: 59
- **Coverage**: Memory, process, filesystem, network, drivers, etc.

### mini-ltp Kernel Compatibility Tests
- **Test Count**: 25
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
