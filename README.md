# Rux

<div align="center">

**A Linux-like OS kernel entirely written in Rust**

[![Rust](https://img.shields.io/badge/Rust-stable-orange.svg)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-riscv64-informational.svg)](https://github.com/rust-osdev/rust-embedded)
[![Tests](https://img.shields.io/badge/tests-4%2C125%20cases-brightgreen.svg)](#-test-status)
[![Verification](https://img.shields.io/badge/verification-4%20tools-brightgreen.svg)](#-formal-verification)
[![Code](https://img.shields.io/badge/code-112%2C000%20lines-blue.svg)](docs/architecture/structure.md)

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
| **Lines of Code** | ~112,000 lines | [Code Structure](docs/architecture/structure.md) |
| **Source Files** | 278 files (274 Rust + 3 ASM + 1 LD) | [Project Structure](docs/architecture/structure.md) |
| **Kernel Unit Tests** | 60 files, 995 cases (last report: 901 PASS + 94 SKIP) | [Unit Test Report](docs/test/unit-test-report.md) |
| **Formal Verification** | 4 tools, 1,116+ test functions; 157 Kani proofs | [Verification Design](docs/development/formal-verification.md) |
| **Smoke Tests** | 15 tests (all passing) | [Testing Guide](docs/test/testing.md) |
| **Linux LTP** | 1,838 official tests | [Testing Guide](docs/test/testing.md) |
| **Platform Support** | RISC-V 64-bit | [Roadmap](docs/progress/roadmap.md) |
| **Syscall Numbers** | 344 dispatched | [Roadmap](docs/progress/roadmap.md) |

**Module Distribution**:
- Filesystem (fs/): 24,478 lines (21.9%)
- System Calls (syscall/): 14,061 lines (12.6%)
- Memory Management (mm/): 10,632 lines (9.5%)
- Unit Tests (tests/): 9,610 lines (8.6%)
- Device Drivers (drivers/): 9,504 lines (8.5%)
- Architecture (arch/): 8,422 lines (7.5%)
- Top-level: 8,396 lines (7.5%)
- Network Stack (net/): 6,905 lines (6.2%)
- Process Management (process/): 5,818 lines (5.2%)
- Process Scheduling (sched/): 4,307 lines (3.8%)
- IPC (ipc/): 4,055 lines (3.6%)
- Sync Primitives (sync/): 3,053 lines (2.7%)
- Interrupt (interrupt/): 1,762 lines (1.6%)
- Diagnostics (dfx/): 1,044 lines (0.9%)

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

# Build userspace programs (shell, apps, toybox)
make user

# Build Rootfs image
make rootfs

# Run kernel (default shell)
make run

# Run unit tests
make test

# Run formal verification (sync check + proptest)
make verify

# Run Kani symbolic verification (all inputs, SAT/SMT)
make kani

# Run Miri UB detection
make miri

# Run SPIN concurrency model checking
make spin
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
security:         capability LSM initialized         [ok]
sched:            CFS scheduler v1                   [ok]
sched:            runqueue per-CPU                   [ok]
sched:            PID allocator init                 [ok]
sched:            idle task (PID 0)                  [ok]
mm:               PCP cpu1 hotpage                   [ok]
softirq:          ksoftirqd per-CPU threads          [ok]
mm:               kswapd reclaim thread              [ok]
dfx:              diagnostic subsystem               [ok]
ipc:              System V + POSIX MQ                [ok]
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

---

## 📁 Project Structure

```
Rux/
├── kernel/                 # Kernel source (~112,000 lines)
│   ├── src/
│   │   ├── fs/           # Filesystem (24,478 lines)
│   │   │   ├── ext4/     # ext4 filesystem
│   │   │   ├── jbd2/     # JBD2 journaling layer
│   │   │   ├── devfs/    # devfs device filesystem
│   │   │   └── procfs/   # procfs process filesystem
│   │   ├── arch/         # RISC-V architecture (8,422 lines)
│   │   │   ├── mm/       # Arch-specific MM (pt, fixmap, ASID, page fault)
│   │   │   ├── boot.S    # MMU trampoline, VMA/LMA linking
│   │   │   ├── trap.S    # PtRegs save/restore, ret_from_fork
│   │   │   └── uaccess.S # User memory access assembly
│   │   ├── drivers/      # Device drivers (9,504 lines)
│   │   │   ├── gpu/      # GPU/framebuffer drivers
│   │   │   ├── input/    # Input device drivers
│   │   │   ├── virtio/   # VirtIO devices (blk/net/gpu/input)
│   │   │   └── net/      # Network devices
│   │   ├── mm/           # Memory management (10,632 lines)
│   │   │   ├── Zone allocator (DMA/DMA32/NORMAL/MOVABLE)
│   │   │   ├── vmemmap, buddy, slab, PCP, memblock
│   │   │   ├── VMA, mm_struct, page fault, COW
│   │   │   └── rmap, hugepage, meminfo
│   │   ├── tests/        # Unit tests (60 files, 995 cases)
│   │   ├── syscall/      # System calls (14,061 lines, 344 syscalls)
│   │   ├── ipc/          # IPC (4,055 lines) — System V, POSIX MQ
│   │   ├── net/          # Network stack (6,905 lines)
│   │   ├── sched/        # Process scheduling (4,307 lines)
│   │   │   ├── CFS, RT (FIFO/RR), Deadline (EDF+CBS), Idle
│   │   ├── process/      # Process management (5,818 lines)
│   │   ├── sync/         # Sync primitives (3,053 lines)
│   │   ├── interrupt/    # Interrupt subsystem (1,762 lines)
│   │   └── dfx/          # Diagnostics/DFX (1,044 lines)
│   └── build.rs          # Build script
├── userspace/            # Userspace programs
│   ├── mrsh/             # mrsh (minimal POSIX shell, musl libc)
│   ├── apps/             # GUI applications (desktop, calculator, clock, vshell)
│   ├── libs/             # Userspace libraries (gui)
│   ├── tests/smoke_test/ # Smoke tests (15 tests, all passing)
│   ├── linux-ltp/        # Official LTP tests (1,838)
│   └── toybox/           # Toybox (200+ command line tools)
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
- **Memory Management**: Sv39 page table/Zone allocator/vmemmap/PCP/COW/Demand paging/ASID/MAP_PRIVATE COW/Swap/LRU page cache/OOM killer
- **Filesystem**: ext4/procfs/devfs/ramfs/JBD2 journaling/crash recovery
- **IPC**: System V semaphores/message queues/shared memory, POSIX message queues
- **Device Drivers**: VirtIO-blk/net/gpu/input, framebuffer, evdev
- **Network Stack**: TCP/UDP/IPv4/ARP/Socket API/IO_uring
- **SMP Multi-core**: 4-core support/load balancing/IPI/per-CPU idle tasks
- **Linux-Style Boot**: MMU trampoline/VMA-LMA linking/PtRegs at stack top
- **Security**: Capabilities/LSM framework/signal/file/IPC permission checks
- **POSIX Timers**: timer_create/settime/gettime/delete, setitimer/getitimer, timerfd

### System Calls

Supports 344 Linux system calls, including:
- File: openat/close/read/write/readv/writev/pread64/pwrite64/lseek/fstat/getdents64/mkdirat/rmdir/unlinkat/sendfile/statfs/copy_file_range/statx
- Process: fork/execve/wait4/exit/getpid/getppid/gettid/kill/clone/sched_yield/prctl/getrusage
- Memory: brk (expand+shrink)/mmap/munmap (MAP_PRIVATE COW)/mprotect/mremap/madvise/msync
- Signal: sigaction/sigprocmask/sigreturn/sigaltstack/sigpending/sigtimedwait
- Network: socket/bind/listen/accept/connect/sendto/recvfrom/sendmsg/recvmsg
- IPC: pipe/pipe2/dup/dup3/select/poll/epoll/eventfd/futex/shmget/shmat/shmdt/msgget/msgsnd/msgrcv/semget/semop/mq_open/mq_send/mq_receive
- Async I/O: io_uring_setup/io_uring_enter/io_uring_register
- Timers: timer_create/timer_settime/timer_gettime/timer_delete/timer_getoverrun/timerfd_create/timerfd_settime/timerfd_gettime/setitimer/getitimer

---

## 📚 Documentation

### Core Documentation

- **[Getting Started](docs/guides/getting-started.md)** - Up and running in 5 minutes
- **[Roadmap](docs/progress/roadmap.md)** - Phase planning and current status (Phase 52)
- **[Project Structure](docs/architecture/structure.md)** - Source code organization
- **[Design Principles](docs/architecture/design.md)** - POSIX compatibility and Linux ABI alignment

### Architecture Documentation

- **[RISC-V Architecture](docs/architecture/riscv64.md)** - RV64GC support details
- **[Boot Process](docs/architecture/boot.md)** - MMU trampoline, VMA/LMA linking, page table init
- **[Memory Management](docs/architecture/memory.md)** - Zone allocator, vmemmap, COW, demand paging
- **[Lock Ordering](docs/architecture/lock-ordering.md)** - Kernel lock hierarchy and nesting rules
- **[Changelog](docs/progress/changelog.md)** - Version history and update records

### Development Guides

- **[Development Workflow](docs/guides/development.md)** - Contributing code and development standards
- **[Boot Process](docs/architecture/boot.md)** - From OpenSBI to kernel boot
- **[User Programs](docs/archive/user-programs.md)** - ELF loading and execve (archived)
- **[Formal Verification](docs/development/formal-verification.md)** - 4-layer verification strategy

### Test Reports

- **[Unit Test Report](docs/test/unit-test-report.md)** - Kernel unit test cases (60 files, 901 PASS + 94 SKIP at last report)
- **[Formal Verification Report](docs/test/formal-verification-report.md)** - proptest-based invariant tests

---

## 🧪 Test Status

**Total: 4,125 test cases + 161 formal verification proofs**

| Test Suite | Cases | Run Command | Environment |
|------------|-------|-------------|-------------|
| **Kernel Unit Tests** | 995 | `make test` | QEMU (no_std, custom harness) |
| **Formal Verification** | 1,116 | `make verify` | Host (std, proptest) |
| **Linux LTP** | 1,838 | `make run` → `/test/linux-ltp/run_ltp.sh` | QEMU |
| **Smoke Tests** | 15 | `make run` → `/test/smoke_test` | QEMU |
| **Kani Proofs** | 157 | `make kani` | Host (Kani/CBMC, all-input symbolic) |
| **SPIN Models** | 4 | `make spin` | Host (SPIN/Promela, concurrency) |
| **Miri UB Detection** | - | `make miri` | Host (Miri, undefined behavior) |

### Kernel Unit Tests (995 cases at last report, 60 files)
- **Framework**: Custom `no_std` harness (`test_pass`, `test_fail`, `test_assert!`)
- **Coverage**: Memory management, process management, filesystem, network, drivers, syscalls, IPC, scheduler, synchronization
- **Report**: [Unit Test Report](docs/test/unit-test-report.md)

### Formal Verification

A 4-layer verification strategy covering ~15% of the kernel's unsafe TCB:

| Layer | Tool | What It Verifies | Cases |
|-------|------|-----------------|-------|
| **L1: Property Testing** | proptest | Data structure invariants (randomized) | 1,088 |
| **L2: Symbolic Verification** | Kani | Core unsafe safety (all inputs, SAT/SMT) | 157 harnesses |
| **L3: Concurrency** | SPIN/Promela | Deadlock-free, no lost wakeup, preempt balance | 4 models |
| **L4: UB Detection** | Miri | Undefined behavior in test code | CI gate |

- **Design**: [Formal Verification Design](docs/development/formal-verification.md)
- **proptest Report**: [Formal Verification Report](docs/test/formal-verification-report.md)

#### Kani Symbolic Verification (157 harnesses, 22 modules)

Proves properties hold for ALL possible inputs via SAT/SMT solvers:
- **mm** (18): slab, page_flags, buddy_alloc, refcount, vma
- **sync** (2): spinlock try_lock/unlock
- **arch** (17): pt_regs, memory_layout, asid
- **process** (16): exit_status, pid, task_state, cred
- **signal** (17): signal bitmap, sigpending
- **drivers** (17): pci, virtio, netdev, input
- **ipc** (5): ipc_id
- **fs** (20): dev_t, permission, stat, inode
- **net** (15): checksum, ethernet, tcp_state
- **sched** (12): rt_bitmap, class
- **interrupt** (12): preempt, softirq
- **security** (9): capability bitmask
- **errno** (5): enum consistency

#### SPIN Concurrency Models (4 models, 8 LTL properties)

Verifies lock ordering and concurrency safety:
- **futex_wait_wake**: No lost wakeup, no spurious sleep
- **lock_ordering**: No deadlock cycle across 5 lock levels
- **interrupt_preempt**: preempt_count bounded, no underflow
- **sched_enqueue_dequeue**: nr_running consistency

#### proptest (1,088 cases, 98 modules)
- **Framework**: [proptest](https://crates.io/crates/proptest) 1.5 (property-based, randomized, 256 cases per test)
- **Subsystems**: mm (252), fs (240), net (123), security (38), interrupt (38), sync (50), sched (70), signal (30), drivers (34), ipc (22), process (39), arch (13), errno (8)
- **Report**: [Formal Verification Report](docs/test/formal-verification-report.md)

### Smoke Tests (15 tests, all passing)
- **Coverage**: File I/O, process management, memory, signals, O_CLOEXEC, sendfile, wait4, process groups, setsid, credentials, readv/writev, gettid, pwrite64, dup3, kill, statfs, sched_yield

### Linux LTP Test Suite (1,838 tests)
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
