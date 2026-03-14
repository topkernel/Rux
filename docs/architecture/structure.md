# Rux Kernel Project Structure

This document describes the directory structure and file organization of the Rux kernel project.

---

## Code Statistics

**Last Updated**: 2026-03-15

### Overall Statistics

| Metric | Value |
|--------|-------|
| **Total Rust Source Files** | 196 |
| **Total Lines of Code** | **~63,200 lines** |
| **Kernel Size (debug)** | ~3 MB |

### Module Code Distribution

| Module | Lines of Code | Percentage | Description |
|--------|---------------|------------|-------------|
| **fs/** | 15,334 | 24.3% | File system |
| **drivers/** | 8,005 | 12.7% | Device drivers |
| **tests/** | 7,376 | 11.7% | Unit tests |
| **net/** | 5,177 | 8.2% | Network protocol stack |
| **syscall/** | 5,654 | 9.0% | System call dispatch |
| **arch/** | 5,168 | 8.2% | Architecture-specific (RISC-V) |
| **mm/** | 4,268 | 6.8% | Memory management |
| **sched/** | 4,255 | 6.7% | Process scheduling |
| **process/** | 2,549 | 4.0% | Process management |
| **sync/** | 1,147 | 1.8% | Synchronization primitives |
| **Other** | ~4,200 | 6.6% | Main entry, configuration, etc. |

### Test Statistics

| Test Type | Count | Description |
|-----------|-------|-------------|
| **Kernel Unit Tests** | 51 files | Memory, process, file system, network, etc. |
| **mini-ltp Tests** | 24 tests | Kernel compatibility tests |
| **Linux LTP Tests** | 1,838 tests | Official LTP test suite (syscall, mem, fs, etc.) |
| **Total** | **1,913 tests** | Comprehensive kernel compatibility coverage |

---

## Directory Structure

```
Rux/
+-- build/                  # Build and configuration tools
|   +-- Makefile           # Build script
|   +-- menuconfig.sh      # Interactive configuration tool
|   +-- config-demo.sh     # Configuration demo script
|
+-- test/                   # Test and debug scripts
|   +-- run.sh             # Quick run script
|   +-- mkrootfs.sh        # Create rootfs image
|   +-- rootfs.img         # ext4 rootfs image (128MB)
|
+-- userspace/              # Userspace programs
|   +-- shell/              # Shell (musl libc)
|   |   +-- src/main.rs     # Shell main program
|   |   +-- Makefile        # Build script
|   |   +-- user.ld         # Linker script
|   |
|   +-- apps/               # GUI applications (musl libc)
|   |   +-- desktop/        # Desktop environment
|   |   +-- calculator/     # Calculator
|   |   +-- clock/          # Clock
|   |   +-- vshell/         # Visual Shell
|   |
|   +-- libs/               # Shared libraries
|   |   +-- gui/            # GUI library (rux_gui)
|   |
|   +-- tests/              # Userspace test programs
|   |   +-- fork_test/      # fork test
|   |   +-- mini-ltp/       # Kernel compatibility test suite
|   |       +-- src/        # Test source code (24 C files)
|   |       +-- output/     # Build output
|   |       |   +-- bin/    # Test binaries
|   |       |   +-- run_tests.sh  # Test run script
|   |       +-- build.sh    # Build script
|   |
|   +-- linux-ltp/          # Official LTP test suite (1,838 tests)
|   |   +-- ltp-20240524.tar.xz  # LTP source tarball
|   |   +-- output/         # Build output
|   |   |   +-- testcases/bin/   # Test binaries (1,838 files)
|   |   |   +-- run_ltp.sh       # Full test runner
|   |   |   +-- run_quick.sh     # Quick test runner
|   |   |   +-- run_syscalls.sh  # Syscall tests runner
|   |   +-- build.sh        # Build script (musl cross-compile)
|   |   +-- README.md       # LTP documentation
|   |
|   +-- toybox/             # Toybox (BusyBox replacement)
|   |   +-- toybox/         # Toybox source
|   |   +-- build-toybox.sh # Build script
|   |
|   +-- build               # User program build script
|   +-- Cargo.toml          # Cargo configuration
|   +-- README.md           # User program documentation
|
+-- toolchain/              # Toolchain
|   +-- build-musl.sh       # musl libc build script
|   +-- riscv64-rux-linux-musl/ # musl toolchain installation directory
|       +-- include/        # musl headers
|       +-- lib/            # musl static libraries
|
+-- docs/                   # Project documentation
|   +-- CLAUDE.md          # AI assistant development guide
|   +-- architecture/      # Architecture documentation
|   |   +-- riscv64.md     # RISC-V architecture documentation
|   |   +-- structure.md   # This file - directory structure documentation
|   +-- development/       # Development documentation
|   |   +-- changelog.md   # Change log
|   |   +-- user-programs.md # User program guide
|   +-- progress/          # Progress documentation
|   |   +-- roadmap.md     # Development roadmap
|   +-- guides/            # Guide documentation
|       +-- getting-started.md # Quick start
|
+-- kernel/                 # Kernel source code
|   +-- src/               # Rust source code
|   |   +-- arch/         # Architecture-specific code
|   |   |   +-- riscv64/  # RISC-V architecture implementation
|   |   |       +-- mod.rs       # Module export
|   |   |       +-- boot.S       # Boot code (assembly)
|   |   |       +-- trap.S       # Exception vector table (assembly)
|   |   |       +-- boot.rs      # Initialization
|   |   |       +-- trap.rs      # Exception handling
|   |   |       +-- mm.rs        # Memory management
|   |   |       +-- smp.rs       # Multi-core support
|   |   |       +-- ipi.rs       # Inter-processor interrupt
|   |   |       +-- context.rs   # Context switching
|   |   |       +-- cpu.rs       # CPU operations
|   |   |
|   |   +-- syscall/      # System call dispatch
|   |   |   +-- mod.rs       # Module export
|   |   |   +-- dispatch.rs  # System call dispatcher
|   |   |   +-- file.rs      # File system calls
|   |   |   +-- process.rs   # Process system calls
|   |   |   +-- memory.rs    # Memory system calls
|   |   |   +-- sched.rs     # Scheduler system calls
|   |   |   +-- signal.rs    # Signal system calls
|   |   |   +-- network.rs   # Network system calls
|   |   |   +-- io.rs        # I/O system calls
|   |   |   +-- time.rs      # Time system calls
|   |   |   +-- misc.rs      # Other system calls
|   |   |
|   |   +-- drivers/      # Device drivers
|   |   |   +-- mod.rs       # Driver module export
|   |   |   +-- intc/        # Interrupt controller
|   |   |   |   +-- mod.rs
|   |   |   |   +-- plic.rs     # RISC-V PLIC driver
|   |   |   |   +-- clint.rs    # RISC-V CLINT driver
|   |   |   +-- timer/       # Timer driver
|   |   |   |   +-- mod.rs
|   |   |   |   +-- riscv64.rs  # RISC-V timer
|   |   |   +-- virtio/      # VirtIO framework
|   |   |   |   +-- mod.rs      # VirtIO module
|   |   |   |   +-- queue.rs    # VirtQueue implementation
|   |   |   |   +-- probe.rs    # Device probing
|   |   |   |   +-- offset.rs   # Register offset definitions
|   |   |   |   +-- virtio_pci.rs # PCI transport layer
|   |   |   +-- blkdev/      # Block devices
|   |   |   |   +-- mod.rs      # VirtIO-blk driver
|   |   |   +-- input/       # Input devices
|   |   |   |   +-- mod.rs
|   |   |   |   +-- evdev.rs    # evdev driver
|   |   |   |   +-- event.rs    # Input event definitions
|   |   |   |   +-- ps2.rs      # PS/2 keyboard/mouse
|   |   |   |   +-- virtio_input.rs # VirtIO input device
|   |   |   +-- net/         # Network devices
|   |   |   |   +-- mod.rs
|   |   |   |   +-- space.rs    # Network device base class
|   |   |   |   +-- loopback.rs # Loopback device
|   |   |   |   +-- virtio_net.rs # VirtIO-net driver
|   |   |   +-- gpu/         # GPU/display devices
|   |   |   |   +-- mod.rs
|   |   |   |   +-- framebuffer.rs # Framebuffer core
|   |   |   |   +-- fb_simple.rs   # Simple framebuffer driver
|   |   |   |   +-- fbdev.rs       # fbdev device interface
|   |   |   |   +-- virtio_gpu.rs  # VirtIO-GPU driver
|   |   |   |   +-- virtio_cmd.rs  # GPU command handling
|   |   |   +-- pci/         # PCI bus
|   |   |       +-- mod.rs      # PCI enumeration and drivers
|   |   |
|   |   +-- mm/           # Memory management
|   |   |   +-- mod.rs       # Module export
|   |   |   +-- page.rs      # Page management (PhysFrame/VirtPage)
|   |   |   +-- page_desc.rs # Page descriptor
|   |   |   +-- allocator.rs # Heap allocator interface
|   |   |   +-- buddy_allocator.rs # Buddy allocator
|   |   |   +-- slab.rs      # Slab allocator
|   |   |   +-- pcp.rs       # Per-CPU page cache
|   |   |   +-- pagemap.rs   # Page table management (platform-independent interface)
|   |   |   +-- mm_struct.rs # Process memory descriptor
|   |   |   +-- vma.rs       # Virtual memory area
|   |   |   +-- meminfo.rs   # Memory info interface
|   |   |
|   |   +-- fs/           # File system
|   |   |   +-- mod.rs       # Module export
|   |   |   +-- vfs.rs       # Virtual file system
|   |   |   +-- file.rs      # File descriptor
|   |   |   +-- inode.rs     # Inode cache
|   |   |   +-- dentry.rs    # Directory entry cache
|   |   |   +-- buffer.rs    # Block cache
|   |   |   +-- bio.rs       # Block I/O layer
|   |   |   +-- mount.rs     # Mount management
|   |   |   +-- superblock.rs # Superblock
|   |   |   +-- path.rs      # Path resolution
|   |   |   +-- stat.rs      # File status structure
|   |   |   +-- rootfs.rs    # Root file system
|   |   |   +-- pipe.rs      # Pipe implementation
|   |   |   +-- char_dev.rs  # Character device
|   |   |   +-- elf.rs       # ELF loader
|   |   |   +-- dev_t.rs     # Device number definitions
|   |   |   +-- devfs/       # devfs file system
|   |   |   |   +-- mod.rs
|   |   |   |   +-- registry.rs # Device registry
|   |   |   +-- procfs.rs    # procfs file system
|   |   |   +-- ext4/        # ext4 file system
|   |   |       +-- mod.rs      # ext4 main module
|   |   |       +-- superblock.rs # Superblock parsing
|   |   |       +-- inode.rs    # Inode structure
|   |   |       +-- file.rs     # File operations
|   |   |       +-- dir.rs      # Directory operations
|   |   |       +-- allocator.rs # Block/Inode allocator
|   |   |       +-- extent.rs   # Extent tree
|   |   |       +-- indirect.rs # Indirect blocks
|   |   |
|   |   +-- net/          # Network protocol stack
|   |   |   +-- mod.rs       # Network module
|   |   |   +-- buffer.rs    # SkBuff (network buffer)
|   |   |   +-- socket.rs    # Socket layer
|   |   |   +-- ethernet.rs  # Ethernet layer
|   |   |   +-- arp.rs       # ARP protocol
|   |   |   +-- ipv4/        # IPv4 protocol
|   |   |   |   +-- mod.rs
|   |   |   |   +-- route.rs   # Routing table
|   |   |   |   +-- checksum.rs # IP checksum
|   |   |   +-- tcp.rs       # TCP protocol
|   |   |   +-- udp.rs       # UDP protocol
|   |   |
|   |   +-- process/      # Process management
|   |   |   +-- mod.rs       # Module export
|   |   |   +-- task.rs      # Task control block
|   |   |   +-- fork.rs      # fork implementation
|   |   |   +-- pid.rs       # PID management
|   |   |   +-- usermod.rs   # User mode management
|   |   |   +-- wait.rs      # wait4 system call
|   |   |
|   |   +-- sched/        # Process scheduling
|   |   |   +-- mod.rs       # Module export
|   |   |   +-- sched.rs     # Scheduler
|   |   |   +-- cfs.rs       # CFS scheduler
|   |   |
|   |   +-- sync/         # Synchronization primitives
|   |   |   +-- mod.rs       # Module export
|   |   |   +-- mutex.rs     # Mutex lock
|   |   |   +-- semaphore.rs # Semaphore
|   |   |   +-- condvar.rs   # Condition variable
|   |   |   +-- futex.rs     # Fast Userspace Mutex
|   |   |
|   |   +-- tests/        # Unit tests (51 test files)
|   |   |   +-- mod.rs       # Test framework entry
|   |   |   |
|   |   |   |  # Memory tests
|   |   |   +-- heap_allocator.rs    # Heap allocator test
|   |   |   +-- page_allocator.rs    # Page allocator test
|   |   |   +-- standard_alloc.rs    # Standard allocator test
|   |   |   +-- mem_mmap.rs          # mmap test
|   |   |   +-- mem_cow.rs           # COW test
|   |   |   |
|   |   |   |  # Process/scheduling tests
|   |   |   +-- fork.rs              # fork test
|   |   |   +-- getpid.rs            # getpid test
|   |   |   +-- wait4.rs             # wait4 test
|   |   |   +-- process_tree.rs      # Process tree test
|   |   |   +-- scheduler.rs         # Scheduler test
|   |   |   +-- preemptive_scheduler.rs # Preemptive scheduling test
|   |   |   +-- sleep_wakeup.rs      # Sleep/wakeup test
|   |   |   +-- smp.rs               # SMP test
|   |   |   +-- smp_schedule.rs      # SMP scheduling test
|   |   |   |
|   |   |   |  # File system tests
|   |   |   +-- file_open.rs         # File open test
|   |   |   +-- file_flags.rs        # File flags test
|   |   |   +-- fdtable.rs           # fd table test
|   |   |   +-- path.rs              # Path resolution test
|   |   |   +-- dcache.rs            # Directory cache test
|   |   |   +-- icache.rs            # Inode cache test
|   |   |   +-- link.rs              # Link test
|   |   |   +-- fcntl.rs             # fcntl test
|   |   |   +-- fstat.rs             # fstat test
|   |   |   +-- mkdir_unlink.rs      # mkdir/unlink test
|   |   |   +-- ext4_allocator.rs    # ext4 allocator test
|   |   |   +-- ext4_file_write.rs   # ext4 file write test
|   |   |   +-- ext4_indirect_blocks.rs # ext4 indirect block test
|   |   |   |
|   |   |   |  # IPC tests
|   |   |   +-- pipe2.rs             # Pipe test
|   |   |   +-- ipc_poll.rs          # poll test
|   |   |   +-- ipc_epoll.rs         # epoll test
|   |   |   +-- ipc_eventfd.rs       # eventfd test
|   |   |   |
|   |   |   |  # Signal tests
|   |   |   +-- signal.rs            # Signal test
|   |   |   +-- signal_procmask.rs   # Signal mask test
|   |   |   |
|   |   |   |  # Network tests
|   |   |   +-- network.rs           # Network basic test
|   |   |   +-- tcp_handshake.rs     # TCP handshake test
|   |   |   |
|   |   |   |  # Driver tests
|   |   |   +-- virtio_queue.rs      # VirtIO queue test
|   |   |   +-- virtio_net.rs        # VirtIO network test
|   |   |   +-- framebuffer.rs       # Framebuffer test
|   |   |   |
|   |   |   |  # System call tests
|   |   |   +-- syscall_file.rs      # File system call test
|   |   |   +-- syscall_memory.rs    # Memory system call test
|   |   |   +-- syscall_process.rs   # Process system call test
|   |   |   +-- syscall_sched.rs     # Scheduler system call test
|   |   |   +-- syscall_signal.rs    # Signal system call test
|   |   |   +-- syscall_network.rs   # Network system call test
|   |   |   +-- syscall_io.rs        # I/O system call test
|   |   |   +-- syscall_time.rs      # Time system call test
|   |   |   +-- syscall_misc.rs      # Misc system call test
|   |   |   +-- user_syscall.rs      # Userspace system call test
|   |   |   +-- execve.rs            # execve test
|   |   |   |
|   |   |   |  # Other tests
|   |   |   +-- listhead.rs          # List test
|   |   |   +-- boundary.rs          # Boundary test
|   |   |   +-- quick.rs             # Quick test entry
|   |   |
|   |   +-- console.rs    # Console (UART)
|   |   +-- config.rs     # Auto-generated config (do not edit manually)
|   |   +-- main.rs       # Kernel entry
|   |   +-- init.rs       # Kernel initialization
|   |   +-- print.rs      # Print macros
|   |   +-- errno.rs      # Error code definitions
|   |
|   +-- build.rs          # Build script (generates config.rs)
|   +-- Cargo.toml        # Kernel crate configuration
|
+-- .cargo/                 # Cargo configuration
|   +-- config.toml       # Cargo tool configuration
|
+-- target/                 # Build output (git ignored)
|   +-- riscv64gc-unknown-none-elf/
|       +-- debug/        # Debug build
|       +-- release/      # Release build
|
+-- Kernel.toml            # Kernel configuration file
+-- Cargo.toml             # Workspace configuration
+-- Cargo.lock             # Dependency lock
+-- Makefile               # Project root Makefile
+-- README.md              # Project description
+-- CLAUDE.md              # AI assistant development guide
+-- LICENSE                # License (MIT)
+-- .gitignore             # Git ignore rules
```

---

## Directory Descriptions

### userspace/ - Userspace Programs

Userspace program directory, containing Shell, GUI applications, test programs, and utilities:

```
userspace/
+-- shell/              # Shell (no_std Rust + musl libc)
|   +-- shell           # Compiled binary
|
+-- apps/               # GUI applications (musl libc)
|   +-- desktop/        # Desktop environment
|   +-- calculator/     # Calculator
|   +-- clock/          # Clock
|   +-- vshell/         # Visual Shell
|
+-- libs/               # Shared libraries
|   +-- gui/            # GUI library (rux_gui)
|       +-- widget.rs   # Widgets
|       +-- window.rs   # Window management
|       +-- input.rs    # Input handling
|
+-- tests/              # Userspace test programs
|   +-- fork_test/      # fork test
|   +-- mini-ltp/       # Kernel compatibility test suite
|       +-- src/        # Test source code (24 tests)
|       |   +-- test_fork.c
|       |   +-- test_fileio.c
|       |   +-- test_pipe.c
|       |   +-- ...
|       +-- output/
|       |   +-- bin/    # Test binaries
|       |   +-- run_tests.sh
|       +-- build.sh
|
+-- linux-ltp/          # Official LTP test suite (1,838 tests)
|   +-- ltp-20240524.tar.xz  # LTP source tarball
|   +-- output/         # Build output
|   |   +-- testcases/bin/   # Test binaries
|   |   +-- run_ltp.sh       # Full test runner
|   |   +-- run_quick.sh     # Quick test runner
|   |   +-- run_syscalls.sh  # Syscall tests runner
|   +-- build.sh        # Build script (musl cross-compile)
|   +-- README.md       # LTP documentation
|
+-- toybox/             # Toybox (BusyBox replacement)
|   +-- toybox/toybox   # Compiled binary
|
+-- build               # Unified build script
```

### rootfs Directory Structure

Internal structure of rootfs image (`test/rootfs.img`):

```
/
+-- bin/                # Basic commands
|   +-- shell           # Shell
|   +-- sh -> shell     # Shell symlink
|   +-- toybox          # Toybox
|   +-- ls -> toybox    # Common command symlinks
|   +-- cat -> toybox
|   +-- echo -> toybox
|   +-- ...
|
+-- app/                # GUI applications
|   +-- desktop         # Desktop environment
|   +-- calculator      # Calculator
|   +-- clock           # Clock
|   +-- vshell          # Visual Shell
|
+-- test/               # Test programs
|   +-- fork_test       # fork test
|   +-- mini-ltp/       # Kernel compatibility tests
|       +-- bin/        # 24 test binaries
|       +-- run_tests.sh
|   +-- linux-ltp/      # Official LTP tests (1,838 tests)
|       +-- testcases/bin/  # LTP test binaries
|       +-- run_ltp.sh      # Full test runner
|       +-- run_quick.sh    # Quick test runner
|       +-- run_syscalls.sh # Syscall tests runner
|
+-- dev/                # Device files
|   +-- console
|   +-- null
|   +-- zero
|   +-- input/
|   |   +-- event0      # Input device
|   +-- fb0             # Framebuffer
|
+-- proc/               # procfs mount point
+-- tmp/                # Temporary files
+-- var/                # Variable data
+-- etc/                # Configuration files
+-- lib/                # Library files
```

### kernel/ - Kernel Source Code

Core kernel source code, organized by functional modules.

#### kernel/src/syscall/ - System Call Dispatch

System call dispatch module, routing system calls to specific implementations:

| File | Function | System Calls |
|------|----------|--------------|
| **dispatch.rs** | System call dispatcher | All system call entry |
| **file.rs** | File system calls | open, close, read, write, lseek, fstat, mkdir, unlink, chdir, getcwd, etc. |
| **process.rs** | Process system calls | execve, wait4, exit, getpid, getppid, etc. |
| **memory.rs** | Memory system calls | brk, mmap, munmap, mprotect, etc. |
| **sched.rs** | Scheduler system calls | sched_yield, nice, etc. |
| **signal.rs** | Signal system calls | kill, signal, sigprocmask, etc. |
| **network.rs** | Network system calls | socket, bind, listen, accept, connect, send, recv, etc. |
| **io.rs** | I/O system calls | poll, select, epoll, etc. |
| **time.rs** | Time system calls | time, gettimeofday, nanosleep, etc. |
| **misc.rs** | Other system calls | uname, sysinfo, etc. |

#### kernel/src/arch/riscv64/ - RISC-V Architecture

**Important**: Currently **only RISC-V 64-bit architecture is supported**.

| File | Function | Code Lines |
|------|----------|------------|
| **boot.S** | Boot code (assembly) | ~150 lines |
| **trap.S** | Exception vector table (assembly) | ~200 lines |
| **boot.rs** | Initialization | ~20 lines |
| **trap.rs** | Exception handling | ~450 lines |
| **mm.rs** | Architecture-specific memory management | ~1,420 lines |
| **smp.rs** | Multi-core support | ~180 lines |
| **ipi.rs** | Inter-processor interrupts | ~130 lines |
| **context.rs** | Context switching | ~270 lines |
| **cpu.rs** | CPU operations | ~140 lines |

**Architecture Support Status**:
- **RISC-V 64-bit (RV64GC)** - Fully supported, current default platform
- **ARM64 (aarch64)** - Not implemented
- **x86_64** - Not implemented

---

## mini-ltp Test Suite

### Test List

| Test Name | Description |
|-----------|-------------|
| test_fork | Process creation |
| test_getpid | Process ID retrieval |
| test_fileio | File I/O (open/read/write/close) |
| test_pipe | Pipe communication |
| test_dup | File descriptor duplication |
| test_mmap | Memory mapping |
| test_stat | File status retrieval |
| test_mkdir | Directory operations |
| test_lseek | File positioning |
| test_time | Time system calls |
| test_wait | Waiting for child processes |
| test_exit | Process exit |
| test_brk | Heap memory management |
| test_chdir | Directory change |
| test_rename | File renaming |
| test_unlink | File deletion |
| test_access | Access permission check |
| test_writev | Vector I/O |
| test_execve | Program execution |
| test_getuid | User/group ID |
| test_nanosleep | High-precision sleep |
| test_ioctl | Terminal ioctl |
| test_fcntl | File control |
| test_fsync | File synchronization |

### Running Tests

In the Rux system:

```bash
cd /test/mini-ltp
./run_tests.sh
```

---

## Linux LTP Test Suite

The official LTP (Linux Test Project) test suite built with musl libc for comprehensive kernel compatibility testing.

### Compilation Statistics

| Category | Compiled | Expected | Rate |
|----------|----------|----------|------|
| **Total** | **1,838** | 1,826 | **101%** |
| Syscall tests | 1,378 | 1,367 | 101% |
| Memory tests | 108 | 108 | 100% |
| Containers | 46 | 46 | 100% |
| Controllers | 20 | 39 | 51% |
| Filesystem tests | 29 | 29 | 100% |
| Security tests | 24 | 24 | 100% |
| Scheduler tests | 23 | 23 | 100% |
| IO tests | 19 | 19 | 100% |

### Known Limitations

Some tests cannot compile with musl due to glibc-specific structures:

- `fmtmsg` - requires `addseverity()` (glibc extension)
- `ioctl` - requires `struct termio` (glibc specific)
- `timer_create` - requires `struct sigevent._sigev_un` (glibc internal)
- `statx` - requires `stx_mnt_id` field (newer kernel)
- `rt_tgsigqueueinfo` - requires `siginfo_t._sifields` (glibc internal)

### Building

```bash
cd userspace/linux-ltp
./build.sh
```

### Running Tests

In the Rux system:

```bash
# Run quick test suite (essential tests)
/test/linux-ltp/run_quick.sh

# Run syscall tests
/test/linux-ltp/run_syscalls.sh

# Run full LTP suite
/test/linux-ltp/run_ltp.sh
```

### LTP Version

Current version: 20240524

---

## Usage Guide

### Compilation

```bash
# Compile kernel
make build

# Compile user programs (shell, apps, mini-ltp, toybox)
make user

# Create rootfs image
make rootfs
```

### Running

```bash
# Run kernel (default shell)
make run

# Run GUI
make gui
```

### Testing

```bash
# Run kernel unit tests
make test

# Run mini-ltp tests in Rux
cd /test/mini-ltp
./run_tests.sh
```

---

## Notes

1. **config.rs is auto-generated** - Do not manually edit `kernel/src/config.rs`, it is automatically generated by `kernel/build.rs` based on `Kernel.toml`.

2. **Platform Limitations** - Currently only supports RISC-V 64-bit architecture.

3. **System Call Compatibility** - Uses Linux system call numbers, fully POSIX/ABI compatible.

4. **Module Export** - When adding new modules, ensure proper export of required interfaces in the parent module's `mod.rs`.

5. **User Programs** - Use musl libc static linking, compatible with kernel ABI.

---

**Document Version**: v7.0
**Last Updated**: 2026-03-15
**Maintainer**: Rux Development Team
