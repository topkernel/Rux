# Development Workflow

This document records the standard workflow for Rux kernel development, ensuring every code change goes through complete verification and documentation updates.

**Last Updated**: 2026-04-09

## Standard Development Workflow

### 1. Write Code

**Principles**:
- Follow the design principles in [DESIGN.md](../architecture/design.md)
- External interfaces must be 100% compatible with Linux ABI/POSIX standards
- Internal implementation is completely free - use any design approach

**Steps**:
1. Read relevant Linux man pages for interface specifications
2. Understand POSIX standard requirements
3. Implement Rust code with any internal design
4. Add necessary comments and documentation

### 2. Kernel Unit Tests

**Test Framework Location**: `kernel/src/tests/`

**Number of Tests**: 58 test modules

**Test Categories**:

| Category | Test Modules | Description |
|----------|--------------|-------------|
| **File System** | file_open, path, file_flags, fdtable, dcache, icache, fstat, fcntl, mkdir_unlink, link | VFS and ext4 tests |
| **Memory Management** | heap_allocator, page_allocator, mem_mmap, mem_cow | Allocator and COW tests |
| **Process Management** | fork, execve, wait4, process_tree, getpid, boundary | Process lifecycle tests |
| **Scheduler** | scheduler, preemptive_scheduler, smp_schedule, sleep_wakeup | CFS scheduler tests |
| **SMP Multi-core** | smp, smp_schedule | Multi-core boot and scheduling tests |
| **Signal Handling** | signal, signal_procmask | Signal mechanism tests |
| **IPC** | pipe2, ipc_poll, ipc_epoll, ipc_eventfd | Inter-process communication tests |
| **Network** | network, tcp_handshake, virtio_net | Network stack tests |
| **Drivers** | virtio_queue, framebuffer | VirtIO and framebuffer tests |
| **ext4** | ext4_allocator, ext4_file_write | ext4 file system tests |
| **System Calls** | syscall_file, syscall_io, syscall_process, syscall_memory, syscall_time, syscall_network, syscall_sched, syscall_signal, syscall_misc | System call category tests |

**Running Tests**:
```bash
# Build test version
cargo build --package rux --features riscv64,unit-test

# Run tests
qemu-system-riscv64 -M virt -cpu rv64 -m 2G -nographic \
  -kernel target/riscv64gc-unknown-none-elf/debug/rux
```

**Adding New Tests**:

1. Create a new test file in `kernel/src/tests/`:
```rust
// kernel/src/tests/my_feature.rs
use crate::tests::{test_pass, test_fail, test_group_start};

pub fn test_my_feature() {
    test_group_start("my_feature");

    // Test code
    if some_condition {
        test_pass("test_case_1");
    } else {
        test_fail("test_case_1", "reason");
    }
}
```

2. Register in `kernel/src/tests/mod.rs`:
```rust
#[cfg(feature = "unit-test")]
pub mod my_feature;

// Add in run_all_tests()
my_feature::test_my_feature();
```

### 3. User-Space Compatibility Tests (mini-ltp)

**Test Framework Location**: `userspace/tests/mini-ltp/`

**Number of Tests**: 25 C language test programs

**Test List**:

| Test Program | Test Content |
|--------------|--------------|
| test_fileio | File read/write operations |
| test_fork | fork system call |
| test_execve | execve system call |
| test_exit | exit system call |
| test_wait | wait/waitpid system call |
| test_getpid | getpid/getppid system call |
| test_pipe | pipe |
| test_dup | dup/dup2 system call |
| test_mmap | mmap/munmap memory mapping |
| test_brk | brk heap memory adjustment |
| test_lseek | lseek file positioning |
| test_mkdir | mkdir directory creation |
| test_unlink | unlink file deletion |
| test_rename | rename |
| test_stat | stat/lstat file status |
| test_fcntl | fcntl file control |
| test_access | access file access check |
| test_chdir | chdir directory change |
| test_fsync | fsync file synchronization |
| test_ioctl | ioctl device control |
| test_nanosleep | nanosleep nanosecond sleep |
| test_time | time/gettimeofday time |
| test_getuid | getuid/geteuid user ID |
| test_writev | writev vector write |

**Building Tests**:
```bash
cd userspace/tests/mini-ltp
./build.sh
```

**Running Tests**:
```bash
# Build kernel and rootfs
make build && make user && make rootfs

# Start kernel
./test/run.sh

# Run tests in shell
/test/mini-ltp/run_tests.sh
```

**Adding New Tests**:

1. Create C source file `userspace/tests/mini-ltp/src/test_xxx.c`:
```c
#include <stdio.h>
#include <unistd.h>
#include <sys/syscall.h>

int main(void) {
    // Test code
    if (syscall(SYS_xxx, ...) == 0) {
        printf("PASS\n");
        return 0;
    } else {
        printf("FAIL\n");
        return 1;
    }
}
```

2. Run `./build.sh` to compile

3. Update rootfs to add test program

### 4. Full System Testing

**Test Objectives**:
- Verify kernel boots normally
- Verify multi-core support (SMP)
- Verify features work in real environment

**Test Commands**:
```bash
# Build
make build

# Single-core boot test
timeout 3 qemu-system-riscv64 -M virt -cpu rv64 -m 2G \
  -nographic -serial mon:stdio \
  -kernel target/riscv64gc-unknown-none-elf/debug/rux

# Multi-core boot test
timeout 3 qemu-system-riscv64 -M virt -cpu rv64 -m 2G \
  -nographic -serial mon:stdio -smp 4 \
  -kernel target/riscv64gc-unknown-none-elf/debug/rux

# Use test script
./test/run.sh

# GUI test
./test/run.sh gui
```

**Verification Checklist**:
- [ ] Kernel boots successfully
- [ ] All harts initialized (multi-core mode)
- [ ] Test output is correct
- [ ] No panics or hangs

### 5. Update Documentation

**Documents to Update**:

1. **Code Review Record** ([code-review.md](../progress/code-review.md))
   - Mark fixed issues as done
   - Record fix solutions and commit messages
   - Update pending issues list

2. **Roadmap** ([roadmap.md](../progress/roadmap.md))
   - Mark completed tasks
   - Add newly discovered tasks
   - Update progress

3. **Design Documents** (if applicable)
   - [design.md](../architecture/design.md) - Architecture design changes
   - [structure.md](../architecture/structure.md) - Directory structure changes

4. **New Documentation** (if applicable)
   - Documentation for new features
   - Debugging guides
   - Testing guides

### 6. Commit Code

**Pre-commit Checklist**:
```bash
# View changes
git status
git diff

# Build verification
make build

# Run kernel tests
cargo build --package rux --features riscv64,unit-test
qemu-system-riscv64 -M virt -cpu rv64 -m 2G -nographic \
  -kernel target/riscv64gc-unknown-none-elf/debug/rux

# Run full system test
./test/run.sh
```

**Commit Guidelines**:
```bash
git add <files>
git commit -m "<type>: <description>

## Details

### Changes
- Specific change 1
- Specific change 2

### Technical Details
- Technical explanation
- Design decisions

### Verification
- Test 1 passed
- Test 2 passed

### Related Files
- file1.rs
- file2.rs

Co-Authored-By: Claude Opus 4.6 <noreply@anthropic.com>"
```

**Commit Types**:
- `feat`: New feature
- `fix`: Bug fix
- `test`: Test related
- `docs`: Documentation update
- `refactor`: Code refactoring
- `perf`: Performance optimization
- `chore`: Build/toolchain related

## Test System Overview

```
Rux Test System
├── Kernel Unit Tests (kernel/src/tests/)
│   ├── Pure Logic Tests (5 modules)
│   ├── Core Data Structures (5 modules)
│   ├── Memory Management Tests (4 modules)
│   ├── Process Management Tests (8 modules)
│   ├── Scheduler Tests (3 modules)
│   ├── SMP Multi-core Tests (2 modules)
│   ├── Synchronization Tests (2 modules)
│   ├── Signal Handling Tests (2 modules)
│   ├── IPC Tests (4 modules)
│   ├── Network Tests (3 modules)
│   ├── Driver Tests (2 modules)
│   ├── File System Tests (10 modules)
│   ├── Memory Syscall Tests (2 modules)
│   ├── System Call Tests (9 modules)
│   └── Boundary Tests (1 module)
│
├── Formal Verification (kernel/verify/)
│   ├── proptest (1,088 cases, 98 modules)
│   ├── Kani Proofs (157 harnesses, 22 modules)
│   ├── SPIN Models (4 models, 8 LTL properties)
│   └── Miri UB Detection (CI gate)
│
├── User-Space Compatibility Tests (userspace/tests/mini-ltp/)
│   ├── File Operation Tests (8)
│   ├── Process Management Tests (5)
│   ├── Memory Management Tests (2)
│   ├── Time Tests (2)
│   └── Other Tests (7)
│
├── Linux LTP (1,838 official tests)
│
├── Smoke Tests (15 tests, all passing)
│
└── Full System Tests (test/)
    ├── quick_test.sh - Quick boot test
    ├── run_riscv64.sh - Complete run test
    └── debug_riscv.sh - GDB debugging
```

### 7. Formal Verification

**4-layer verification strategy**:

| Layer | Tool | Command | What It Verifies |
|-------|------|---------|-----------------|
| L1: Property Testing | proptest | `make verify` | Data structure invariants (1,088 cases, 98 modules) |
| L2: Symbolic Verification | Kani | `make kani` | Core unsafe safety, all inputs (157 harnesses, 22 modules) |
| L3: Concurrency | SPIN | `make spin` | Deadlock-free, no lost wakeup (4 models, 8 LTL properties) |
| L4: UB Detection | Miri | `make miri` | Undefined behavior in test code (CI gate) |

## Quick Checklist

Before submitting any code, ensure:

- [ ] **Code compiles** (`make build`)
- [ ] **Kernel unit tests pass** (`cargo build --features unit-test`)
- [ ] **Full system boot test passes** (`./test/run.sh`)
- [ ] **Documentation updated** (roadmap.md, etc.)
- [ ] **Clear commit message** (follow commit guidelines)
- [ ] **Linux ABI compliance** (external interfaces must match Linux)
- [ ] **Code review completed** (self-review or peer review)

## Common Mistakes

### Wrong Practices

1. **Build only without testing**
   - Successful compilation does not mean correct functionality
   - Must run tests for verification

2. **Skip documentation updates**
   - Issues in roadmap.md not marked
   - Cannot track issue status in the future

3. **Unclear commit messages**
   - "fix bug" - too brief
   - "update" - no specific content
   - Should explain what, why, and how verified

4. **Breaking Linux ABI compatibility**
   - Changing system call behavior
   - Modifying user-visible data structure layouts
   - External interfaces must be fully compatible with Linux

### Correct Practices

1. **Complete test workflow**
   ```bash
   make build           # Build
   make test            # Run kernel tests
   ./test/run.sh        # Full system test
   ```

2. **Update documentation promptly**
   - Update roadmap.md after fixing issues
   - Update progress after completing features
   - Update design.md for major changes

3. **Clear commit messages**
   ```
   type: brief description (within 50 characters)

   ## Details
   - Change 1
   - Change 2

   ## Verification
   - Tests passed

   Co-Authored-By: Claude Opus 4.6
   ```

4. **Maintain external compatibility**
   - Use Linux system call numbers and data structure layouts
   - Follow POSIX standards for interface behaviors
   - Internal implementation can use any approach

## Related Documents

- [CLAUDE.md](../../CLAUDE.md) - AI Assistant Development Guide
- [design.md](../architecture/design.md) - Design Principles
- [roadmap.md](../progress/roadmap.md) - Development Roadmap
- [testing.md](../test/testing.md) - Testing Guide

## Version History

- **2026-03-04**: Major document update
  - Updated kernel unit test information (58 test modules, 825 test cases)
  - Added user-space compatibility test (mini-ltp) section
  - Updated test system overview
  - Fixed outdated examples and paths
- **2026-02-08**: Created document, recorded standard development workflow
