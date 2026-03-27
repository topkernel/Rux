# Quick Start Guide

Welcome to Rux OS! This guide will help you build and run the Rux kernel in 5 minutes.

## Requirements

### Required Tools

- **Rust Toolchain** (stable or nightly)
  ```bash
  rustc --version
  cargo --version
  ```

- **QEMU System Emulator**
  ```bash
  qemu-system-riscv64 --version  # At least version 5.0
  ```

- **RISC-V Cross-Compilation Toolchain** (for user programs)
  ```bash
  riscv64-linux-gnu-gcc --version
  ```

### Optional Tools

- **GDB Debugger** (for debugging)
  ```bash
  riscv64-unknown-elf-gdb --version
  ```

## Quick Build

### 1. Clone the Repository

```bash
git clone https://github.com/topkernel/rux.git
cd rux
```

### 2. Add Rust Targets

```bash
rustup target add riscv64gc-unknown-none-elf
rustup target add riscv64gc-unknown-linux-musl
```

### 3. Build the Project

```bash
# Build the kernel
make build

# Build musl libc SDK (required for user programs)
make sdk

# Build user-space programs (shell, apps, mini-ltp, toybox)
make user

# Build toybox (200+ Linux command line tools)
make toybox

# Build the Rootfs image
make rootfs
```

### 4. Run the Kernel

```bash
# Run the kernel (default shell)
make run

# Run the kernel (toybox shell)
make run-toybox

# Run GUI desktop
make gui

# Run unit tests
make test
```

## Expected Output

If everything is working correctly, you should see:

```
OpenSBI v0.9
   ____                    _____ ____ _____
  / __ \                  / ____|  _ \_   _|
 | |  | |_ __   ___ _ __ | (___ | |_) || |
 | |  | | '_ \ / _ \ '_ \ \___ \|  _ < | |
 | |__| | |_) |  __/ | | |____) | |_) || |_
  \____/| .__/ \___|_| |_|_____/|____/_____|
        | |
        |_|

Platform Name             : riscv-virtio,qemu
Platform HART Count       : 4
...


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
mm:               zone allocator (DMA/DMA32/NORMAL)  [ok]
mm:               buddy allocator order 0-10         [ok]
mm:               vmemmap initialized                [ok]
mm:               PCP per-CPU pagesets               [ok]
boot:             FDT/DTB parsed                     [ok]
mm:               user frame allocator 64MB          [ok]
mm:               32768 page descriptors             [ok]
intc:             PLIC @ 0x0C000000                  [ok]
intc:             external IRQ routing               [ok]
ipi:              SSIP software IRQ                  [ok]
bio:              buffer cache layer                 [ok]
fs:               ext4 driver loaded                 [ok]
fs:               ramfs mounted /                    [ok]
fs:               procfs mounted /proc               [ok]
fs:               devfs mounted /dev                 [ok]
driver:           virtio-blk PCI x1                  [ok]
driver:           virtio-net x1                      [ok]
driver:           virtio-gpu x1                      [ok]
driver:           virtio-input x1                    [ok]
sched:            multi-class scheduler              [ok]
trap:             sie.SEIE enabled                   [ok]
init:             loading /bin/shell                 [ok]
init:             ELF loaded to user space           [ok]
init:             init task (PID 1) enqueued         [ok]


========================================
  Rux OS Shell v0.5 (musl libc)
========================================
Type 'help' for available commands

root#
```

## Common Commands

### Build

```bash
# Build the kernel (debug mode)
make build

# Build the kernel (release mode, optimized)
make build RELEASE=1

# Build musl libc SDK (required before user programs)
make sdk

# Build user-space programs (shell, apps, mini-ltp)
make user

# Build toybox (200+ Linux command line tools)
make toybox

# Build the Rootfs image (shell + toybox)
make rootfs

# Build and run unit tests
make test
```

### Run

```bash
# Run the kernel (default shell)
make run

# Run the kernel (toybox shell)
make run-toybox

# Run GUI desktop
make gui

# GDB debugging
make debug
```

### Configuration

```bash
# Interactive configuration (menuconfig)
make menuconfig

# View current configuration
make config

# Edit configuration file
vim Kernel.toml
```

### Clean

```bash
# Clean kernel build artifacts
make clean

# Complete cleanup
make distclean
```

## Platform Support

### RISC-V 64-bit (Only Supported)

```bash
make build
make run
```

**Note**: ARM64 (aarch64) architecture has been removed and is no longer maintained.

## Using the Shell

After Rux starts, it enters the default shell (musl libc). Supports command history, tab completion, and line editing:

```bash
root# echo "Hello Rux!"
Hello Rux!

root# pid
PID: 1, PPID: 0

root# time
Uptime: 12345 ms

root# help
Built-in commands: echo, help, exit, time, pid

root# ls /
bin  app  test  dev  proc  tmp  var  etc  lib

root# /bin/toybox ls -la /
drwxr-xr-x    1 0        0             0 .
drwxr-xr-x    1 0        0             0 ..
...

root# /bin/toybox cat /proc/cpuinfo
processor       : 0
hart            : 0
isa             : rv64imafdc
mmu             : sv39
...

root# /app/desktop
# Launch desktop environment (requires GUI support)
```

**Important**: `ls` is a shell built-in command. To test fork/exec, use external programs like `/bin/toybox`.

## Running Tests

### Kernel Unit Tests

```bash
make test
```

Test module categories (53 test files):

**Memory Tests**
- heap_allocator, page_allocator, standard_alloc
- mem_mmap, mem_cow

**Process/Scheduler Tests**
- fork, getpid, wait4, process_tree
- scheduler, preemptive_scheduler, sleep_wakeup
- smp, smp_schedule

**File System Tests**
- file_open, file_flags, fdtable, path
- dcache, icache, link, fcntl, fstat, mkdir_unlink
- ext4_allocator, ext4_file_write, ext4_indirect_blocks

**IPC Tests**
- pipe2, ipc_poll, ipc_epoll, ipc_eventfd

**Signal Tests**
- signal, signal_procmask

**Network Tests**
- network, tcp_handshake

**Driver Tests**
- virtio_queue, virtio_net, framebuffer

**System Call Tests**
- syscall_file, syscall_memory, syscall_process
- syscall_sched, syscall_signal, syscall_network
- syscall_io, syscall_time, syscall_misc

### mini-ltp Kernel Compatibility Tests

```bash
# Run in Rux shell
cd /test/mini-ltp
./run_tests.sh
```

25 tests covering core system calls:
- test_fork, test_getpid, test_fileio, test_pipe
- test_dup, test_mmap, test_stat, test_mkdir
- test_lseek, test_time, test_wait, test_exit
- test_brk, test_chdir, test_rename, test_unlink
- test_access, test_writev, test_execve, test_getuid
- test_nanosleep, test_ioctl, test_fcntl, test_fsync
- test_getdents

### Linux LTP Test Suite

```bash
# Build LTP (requires musl SDK)
make ltp

# Run in Rux shell
cd /test/linux-ltp
./run_tests.sh
```

- 1,838 official LTP test binaries
- Compile rate: 101% (musl libc cross-compilation)
- Coverage: Syscalls (1,378), memory (108), containers (46), filesystem (29), security (24), scheduler (23), IO (19)

## Troubleshooting

### Build Errors

**Problem**: Rust target not found
```bash
error: target not found
```

**Solution**:
```bash
rustup target add riscv64gc-unknown-none-elf
rustup target add riscv64gc-unknown-linux-musl
```

**Problem**: Missing cross-compilation toolchain
```bash
riscv64-linux-gnu-gcc: command not found
```

**Solution**:
```bash
# Ubuntu/Debian
sudo apt-get install gcc-riscv64-linux-gnu

# Arch Linux
sudo pacman -S riscv64-linux-gnu-gcc
```

### Runtime Errors

**Problem**: QEMU version too old
```bash
qemu-system-riscv64: unsupported machine
```

**Solution**: Upgrade QEMU to version 5.0 or higher

**Problem**: OpenSBI not found
```bash
qemu-system-riscv64: could not load bootloader
```

**Solution**:
- QEMU >= 5.0 usually includes OpenSBI
- Or manually specify `-bios <path>`

**Problem**: Rootfs image does not exist
```bash
fs: ext4 mount failed
```

**Solution**:
```bash
make sdk
make user
make toybox
make rootfs
```

### Test Timeout

**Problem**: Tests take too long to run

**Solution**:
1. Make sure no other QEMU processes are running:
   ```bash
   pkill qemu
   ```
2. Build in release mode:
   ```bash
   make build RELEASE=1
   ```

### MMU-Related Issues

If you encounter "Load access fault" or "Store access fault":

1. Clean and rebuild:
   ```bash
   make clean && make build
   ```
2. Verify you are using the correct kernel version

## rootfs Directory Structure

```
/
├── bin/          # Basic commands
│   ├── shell     # Shell (musl libc)
│   ├── toybox    # Toybox (200+ Linux commands)
│   └── sh        # Shell symbolic link
├── app/          # GUI applications
│   ├── desktop   # Desktop environment
│   ├── calculator  # Calculator
│   ├── clock     # Clock
│   └── vshell    # Visual Shell
├── test/         # Test programs
│   ├── fork_test
│   ├── linux-ltp # Linux LTP Test Suite
│   └── mini-ltp/ # Kernel compatibility tests (25)
├── dev/          # Device files (devfs)
├── proc/         # Process info (procfs)
├── tmp/          # Temporary files
└── etc/          # Configuration files
```

## Next Steps

- Read [Design Principles](../architecture/design.md)
- Learn about [Code Structure](../architecture/structure.md)
- Understand the [Boot Process](../architecture/boot.md)
- Explore [Memory Management](../architecture/memory.md)
- Check the [Development Workflow](development.md)
- View the [Development Roadmap](../progress/roadmap.md)
- View the [Development Roadmap](../progress/roadmap.md)

## Getting Help

- **Documentation Center**: Return to [Documentation Home](../../README.md)
- **Issue Reporting**: [GitHub Issues](https://github.com/topkernel/rux/issues)

---

Last updated: 2026-03-27
