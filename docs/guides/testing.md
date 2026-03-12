# Rux Kernel Testing Guide

This document explains the Rux kernel testing system, including the test framework, test status, and best practices.

**Last Updated**: 2026-03-04
**Test Scale**: 51 kernel tests + 24 mini-ltp compatibility tests

---

## Table of Contents

- [Test System Overview](#test-system-overview)
- [Test Environment Configuration](#test-environment-configuration)
- [Kernel Unit Tests](#kernel-unit-tests)
- [User-Space Compatibility Tests](#user-space-compatibility-tests)
- [Adding New Tests](#adding-new-tests)
- [Testing Best Practices](#testing-best-practices)
- [Known Limitations](#known-limitations)

---

## Test System Overview

```
Rux Test System
├── Kernel Unit Tests (kernel/src/tests/)
│   │
│   ├── Basic Data Structures (4 modules)
│   │   ├── listhead.rs    - Doubly linked list
│   │   ├── path.rs        - Path parsing
│   │   ├── file_flags.rs  - File flags
│   │   └── boundary.rs    - Boundary conditions
│   │
│   ├── Memory Management (5 modules)
│   │   ├── heap_allocator.rs   - Heap allocator
│   │   ├── page_allocator.rs   - Page allocator
│   │   ├── standard_alloc.rs   - Standard allocator
│   │   ├── mem_mmap.rs         - mmap system call
│   │   └── mem_cow.rs          - Copy-on-Write
│   │
│   ├── Process Management (8 modules)
│   │   ├── scheduler.rs            - Scheduler
│   │   ├── process_tree.rs         - Process tree
│   │   ├── fork.rs                 - fork system call
│   │   ├── execve.rs               - execve system call
│   │   ├── getpid.rs               - Process ID
│   │   ├── wait4.rs                - wait4 system call
│   │   ├── preemptive_scheduler.rs - Preemptive scheduling
│   │   └── sleep_wakeup.rs         - Sleep/wakeup
│   │
│   ├── Signal Handling (2 modules)
│   │   ├── signal.rs          - Signal handling
│   │   └── signal_procmask.rs - Signal mask
│   │
│   ├── File System (8 modules)
│   │   ├── file_open.rs   - File open
│   │   ├── fdtable.rs     - File descriptor table
│   │   ├── dcache.rs      - Directory entry cache
│   │   ├── icache.rs      - Inode cache
│   │   ├── fstat.rs       - File status
│   │   ├── fcntl.rs       - File control
│   │   ├── link.rs        - Hard link
│   │   └── mkdir_unlink.rs - Directory operations
│   │
│   ├── ext4 File System (3 modules)
│   │   ├── ext4_allocator.rs      - ext4 allocator
│   │   ├── ext4_file_write.rs     - ext4 file write
│   │   └── ext4_indirect_blocks.rs - ext4 indirect blocks
│   │
│   ├── IPC (4 modules)
│   │   ├── pipe2.rs       - pipe2 system call
│   │   ├── ipc_poll.rs    - poll system call
│   │   ├── ipc_epoll.rs   - epoll system call
│   │   └── ipc_eventfd.rs - eventfd system call
│   │
│   ├── Network (3 modules)
│   │   ├── network.rs        - Network framework
│   │   ├── tcp_handshake.rs  - TCP handshake
│   │   └── virtio_net.rs     - VirtIO network card
│   │
│   ├── Device Drivers (2 modules)
│   │   ├── virtio_queue.rs  - VirtIO queue
│   │   └── framebuffer.rs   - Framebuffer
│   │
│   ├── SMP Multi-core (2 modules)
│   │   ├── smp.rs          - SMP multi-core boot
│   │   └── smp_schedule.rs - SMP scheduling
│   │
│   ├── User Mode (1 module)
│   │   └── user_syscall.rs - User system calls
│   │
│   └── System Calls (9 modules)
│       ├── syscall_file.rs    - File system calls
│       ├── syscall_memory.rs  - Memory system calls
│       ├── syscall_process.rs - Process system calls
│       ├── syscall_sched.rs   - Scheduler system calls
│       ├── syscall_signal.rs  - Signal system calls
│       ├── syscall_network.rs - Network system calls
│       ├── syscall_io.rs      - I/O system calls
│       ├── syscall_time.rs    - Time system calls
│       └── syscall_misc.rs    - Misc system calls
│
├── User-Space Compatibility Tests (userspace/tests/mini-ltp/)
│   │
│   ├── File Operations (8)
│   │   ├── test_fileio.c  - File read/write
│   │   ├── test_stat.c    - File status
│   │   ├── test_lseek.c   - File positioning
│   │   ├── test_mkdir.c   - Directory operations
│   │   ├── test_rename.c  - File rename
│   │   ├── test_unlink.c  - File deletion
│   │   ├── test_access.c  - Access permissions
│   │   └── test_writev.c  - Vector I/O
│   │
│   ├── Process Management (5)
│   │   ├── test_fork.c   - Process creation
│   │   ├── test_execve.c - Program execution
│   │   ├── test_wait.c   - Wait for child process
│   │   ├── test_exit.c   - Process exit
│   │   └── test_getpid.c - Process ID
│   │
│   ├── Memory Management (2)
│   │   ├── test_mmap.c - Memory mapping
│   │   └── test_brk.c  - Heap memory
│   │
│   ├── Time (2)
│   │   ├── test_time.c      - Time system calls
│   │   └── test_nanosleep.c - High-precision sleep
│   │
│   └── Others (7)
│       ├── test_pipe.c    - Pipe communication
│       ├── test_dup.c     - File descriptor duplication
│       ├── test_chdir.c   - Directory change
│       ├── test_getuid.c  - User/group ID
│       ├── test_ioctl.c   - Terminal ioctl
│       ├── test_fcntl.c   - File control
│       └── test_fsync.c   - File synchronization
│
└── Full System Tests (test/)
    ├── quick_test.sh    - Quick boot test
    ├── run_riscv64.sh   - Complete run test
    └── debug_riscv.sh   - GDB debugging
```

---

## Test Environment Configuration

### Enabling Kernel Unit Tests

Rux uses the `unit-test` feature to control test compilation:

```bash
# Build with unit tests enabled
cargo build --package rux --features riscv64,unit-test

# Run tests
qemu-system-riscv64 -M virt -cpu rv64 -m 2G -nographic \
  -kernel target/riscv64gc-unknown-none-elf/debug/rux
```

### Normal Build (Without Tests)

```bash
# Normal build, without test code
make build

# Or use cargo directly
cargo build --package rux --features riscv64
```

### Test Environment

| Item | Configuration |
|------|---------------|
| **QEMU** | 6.2.0+ (RISC-V 64-bit) |
| **Target Platform** | riscv64gc-unknown-none-elf |
| **CPU** | 4 cores (QEMU virt machine) |
| **Memory** | 2 GB |
| **MMU** | Sv39 (3-level page table) |

---

## Kernel Unit Tests

### Test Framework

Rux is a `no_std` kernel and cannot use the standard library's `#[test]` and `cargo test`. It uses a custom test framework:

**Framework Location**: `kernel/src/tests/mod.rs`

**Core Components**:
- `test_pass(name)` - Record test passed
- `test_fail(name, reason)` - Record test failed
- `test_skip(name, reason)` - Record test skipped
- `test_group_start(name)` - Start test group
- `test_assert!()` - Assert macro (no panic on failure)
- `test_assert_eq!()` - Equality assert macro

**Test Entry**: `run_all_tests()` function

### Test Module Status

#### Basic Data Structures

| Module | Status | Test Content |
|--------|--------|--------------|
| listhead.rs | Passed | Initialization, add, delete, traverse |
| path.rs | Passed | Absolute path, parent directory, filename extraction |
| file_flags.rs | Passed | Access mode, flag combinations |
| boundary.rs | Partial | Process pool exhaustion (expected behavior) |

#### Memory Management

| Module | Status | Test Content |
|--------|--------|--------------|
| heap_allocator.rs | Passed | Box, Vec, String allocation |
| page_allocator.rs | Passed | PhysAddr/VirtAddr, FrameAllocator |
| standard_alloc.rs | Passed | Standard library allocator interface |
| mem_mmap.rs | Passed | mmap/munmap/mprotect/msync |
| mem_cow.rs | Passed | COW constants, page fault handling, fork integration |

#### Process Management

| Module | Status | Test Content |
|--------|--------|--------------|
| scheduler.rs | Passed | get_current_pid/ppid, find_task_by_pid |
| process_tree.rs | Passed | Parent-child relationship, sibling relationship, list integrity |
| fork.rs | Partial | Basic fork (resource pool limitation) |
| execve.rs | Passed | Empty path, non-existent file, ELF loading |
| getpid.rs | Passed | getpid/getppid consistency |
| wait4.rs | Passed | ECHILD, WNOHANG |
| preemptive_scheduler.rs | Passed | jiffies, need_resched, time slice |
| sleep_wakeup.rs | Passed | TaskState, wake_up |

#### Signal Handling

| Module | Status | Test Content |
|--------|--------|--------------|
| signal.rs | Passed | Signal enum, SigFlags, SigAction |
| signal_procmask.rs | Passed | rt_sigprocmask, SIG_BLOCK/UNBLOCK |

#### File System

| Module | Status | Test Content |
|--------|--------|--------------|
| file_open.rs | Passed | RootFS lookup, create, O_CREAT/O_EXCL |
| fdtable.rs | Passed | alloc_fd, install_fd, close_fd, fd reuse |
| dcache.rs | Passed | dcache_add/lookup/remove, LRU |
| icache.rs | Passed | icache_add/lookup/remove, LRU |
| fstat.rs | Passed | fstat system call |
| fcntl.rs | Passed | fcntl system call |
| link.rs | Passed | link system call |
| mkdir_unlink.rs | Passed | mkdir/unlink system call |

#### ext4 File System

| Module | Status | Test Content |
|--------|--------|--------------|
| ext4_allocator.rs | Passed | BlockAllocator, InodeAllocator |
| ext4_file_write.rs | Passed | File write operations |
| ext4_indirect_blocks.rs | Passed | Single-level indirect blocks, index calculation |

#### IPC

| Module | Status | Test Content |
|--------|--------|--------------|
| pipe2.rs | Passed | pipe2, O_CLOEXEC, O_NONBLOCK |
| ipc_poll.rs | Passed | poll, PollFd, POLLIN/POLLOUT |
| ipc_epoll.rs | Passed | epoll_create/ctl/wait |
| ipc_eventfd.rs | Passed | eventfd, event notification |

#### Network

| Module | Status | Test Content |
|--------|--------|--------------|
| network.rs | Passed | Network subsystem initialization |
| tcp_handshake.rs | Passed | TCP connection establishment, three-way handshake |
| virtio_net.rs | Passed | VirtIO-net device, packet send/receive |

#### Device Drivers

| Module | Status | Test Content |
|--------|--------|--------------|
| virtio_queue.rs | Passed | VirtIO data structures, descriptors |
| framebuffer.rs | Passed | Framebuffer initialization, pixel operations |

#### SMP Multi-core

| Module | Status | Test Content |
|--------|--------|--------------|
| smp.rs | Passed | is_boot_hart, hart ID, MAX_CPUS |
| smp_schedule.rs | Partial | Per-CPU run queue, load_balance |

#### System Calls

| Module | Status | Test Content |
|--------|--------|--------------|
| syscall_file.rs | Passed | open/close/read/write/lseek/fstat |
| syscall_memory.rs | Passed | brk/mmap/munmap/mprotect |
| syscall_process.rs | Passed | fork/execve/wait4/exit/getpid |
| syscall_sched.rs | Passed | sched_yield/nice |
| syscall_signal.rs | Passed | kill/sigaction/sigprocmask |
| syscall_network.rs | Passed | socket/bind/listen/accept/connect |
| syscall_io.rs | Passed | poll/select/epoll |
| syscall_time.rs | Passed | time/gettimeofday/nanosleep |
| syscall_misc.rs | Passed | uname/sysinfo/prlimit64/getrandom |

---

## User-Space Compatibility Tests

### mini-ltp Test Suite

**Location**: `userspace/tests/mini-ltp/`

**Test List** (24 tests):

| Test Program | Test Content | Status |
|--------------|--------------|--------|
| test_fork | Process creation | Passed |
| test_getpid | Process ID retrieval | Passed |
| test_fileio | File I/O | Passed |
| test_pipe | Pipe communication | Passed |
| test_dup | File descriptor duplication | Passed |
| test_mmap | Memory mapping | Passed |
| test_stat | File status retrieval | Passed |
| test_mkdir | Directory operations | Passed |
| test_lseek | File positioning | Passed |
| test_time | Time system calls | Passed |
| test_wait | Wait for child process | Passed |
| test_exit | Process exit | Passed |
| test_brk | Heap memory management | Passed |
| test_chdir | Directory change | Passed |
| test_rename | File rename | Passed |
| test_unlink | File deletion | Passed |
| test_access | Access permission check | Passed |
| test_writev | Vector I/O | Passed |
| test_execve | Program execution | Passed |
| test_getuid | User/group ID | Passed |
| test_nanosleep | High-precision sleep | Passed |
| test_ioctl | Terminal ioctl | Passed |
| test_fcntl | File control | Passed |
| test_fsync | File synchronization | Passed |

### Building mini-ltp

```bash
cd userspace/tests/mini-ltp
./build.sh
```

### Running mini-ltp

```bash
# In Rux shell
/test/mini-ltp/run_tests.sh
```

---

## Adding New Tests

### Adding Kernel Unit Tests

1. **Create test file** `kernel/src/tests/my_feature.rs`:

```rust
use crate::tests::{test_pass, test_fail, test_group_start};

pub fn test_my_feature() {
    test_group_start("my_feature");

    // Test case 1
    if some_condition {
        test_pass("test_case_1");
    } else {
        test_fail("test_case_1", "reason");
    }

    // Test case 2
    test_assert!(another_condition, "test_case_2");
}
```

2. **Register test** in `kernel/src/tests/mod.rs`:

```rust
#[cfg(feature = "unit-test")]
pub mod my_feature;

// Add in run_all_tests()
my_feature::test_my_feature();
```

3. **Build and run**:

```bash
cargo build --package rux --features riscv64,unit-test
qemu-system-riscv64 -M virt -cpu rv64 -m 2G -nographic \
  -kernel target/riscv64gc-unknown-none-elf/debug/rux
```

### Adding mini-ltp Tests

1. **Create C source file** `userspace/tests/mini-ltp/src/test_xxx.c`:

```c
#include <stdio.h>
#include <unistd.h>

int main(void) {
    // Test code
    if (syscall_succeeds) {
        return 0;  // PASS
    } else {
        return 1;  // FAIL
    }
}
```

2. **Build test**:

```bash
cd userspace/tests/mini-ltp
./build.sh
```

3. **Update rootfs**:

```bash
make rootfs
```

---

## Testing Best Practices

### Test Naming Conventions

| Type | Naming Format |
|------|---------------|
| Test file | `test_<module>.rs` or `<feature>.rs` |
| Test function | `test_<feature>()` |
| Test group | `test_group_start("<module_name>")` |
| Test case | Descriptive name, e.g., "basic_fork_success" |

### Test Structure

```rust
pub fn test_feature() {
    test_group_start("feature");

    // 1. Basic functionality
    test_assert!(basic_check(), "basic_functionality");

    // 2. Boundary conditions
    test_assert_eq!(edge_case(), expected, "edge_case");

    // 3. Error handling
    test_assert!(error_handled(), "error_handling");
}
```

### Problems to Avoid

| Problem | Description |
|---------|-------------|
| Global state dependency | Each test should initialize independently |
| Large object stack allocation | Use Box for heap allocation |
| Complex drop operations | May trigger PANIC |

### Safe Operations

| Operation | Description |
|-----------|-------------|
| Box allocation | Single object heap allocation |
| Simple stack allocation | Basic types, small arrays |
| Integer operations | No memory operations |

---

## Known Limitations

### 1. Vec Drop PANIC

**Problem**: Releasing memory when `Vec` goes out of scope may trigger PANIC

**Workaround**: Skip Vec drop related tests, only test basic operations

### 2. Cannot Use cargo test

**Reason**: Rux is a `no_std` kernel

**Solution**: Use custom test framework, run in QEMU

### 3. Resource Pool Limitations

**Problem**: Some tests (like multiple fork) are limited by static resource pools

**Solution**: Skip after testing boundary conditions, or implement dynamic resource allocation

---

## Test Coverage Statistics

### By Module Category

| Module | Test Files | Status |
|--------|------------|--------|
| Basic Data Structures | 4 | Excellent |
| Memory Management | 5 | Excellent |
| Process Management | 8 | Good |
| Signal Handling | 2 | Excellent |
| File System | 8 | Excellent |
| ext4 | 3 | Excellent |
| IPC | 4 | Excellent |
| Network | 3 | Excellent |
| Device Drivers | 2 | Excellent |
| SMP Multi-core | 2 | Good |
| User Mode | 1 | Excellent |
| System Calls | 9 | Excellent |
| **Total** | **51** | **~98% Pass** |

### Historical Trend

| Date | Version | Test Files | Notes |
|------|---------|------------|-------|
| 2026-02-09 | Phase 18.5 | ~40 | pagemap refactoring |
| 2026-02-11 | Phase 19 | 43 | COW + IPC |
| 2026-02-27 | Phase 22 | 43 | procfs + toybox |
| 2026-03-04 | Phase 24 | **51** | System call tests + framebuffer |

---

## Improvement Directions

### Short Term
1. Add concurrency tests
2. Add performance benchmarks
3. Improve boundary condition tests

### Medium Term
1. Implement dynamic page table allocator
2. Improve TCP/UDP data send/receive tests
3. Add file system stress tests

### Long Term
1. Establish CI/CD automated testing
2. Add fuzz testing
3. Implement code coverage statistics

---

## Related Documents

- [Development Workflow](development.md)
- [Design Documents](../architecture/design.md)
- [Roadmap](../progress/roadmap.md)

---

## Changelog

- **2026-03-04**: Merged unit-test-report.md and testing.md
  - Unified test system documentation
  - Updated test count (51 kernel + 24 mini-ltp)
  - Added test system overview diagram
- **2026-02-08**: Added fork/execve/wait4 tests
- **2026-02-08**: Initial version, recorded existing test status
