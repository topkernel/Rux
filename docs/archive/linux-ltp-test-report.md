# Linux LTP Compatibility Test Report

Report on the Linux Test Project (LTP) integration for Rux kernel compatibility testing.

**Last Updated**: 2026-04-09
**LTP Version**: 20240524
**Test Scale**: 1,838 compiled test binaries

---

## Table of Contents

- [Overview](#overview)
- [Compilation Statistics](#compilation-statistics)
- [Building](#building)
- [Running](#running)
- [Known Limitations](#known-limitations)
- [Test Categories](#test-categories)
- [Related Documents](#related-documents)
- [Changelog](#changelog)

---

## Overview

The [Linux Test Project (LTP)](https://github.com/linux-test-project/ltp) is the official Linux kernel test suite. Rux integrates LTP to verify Linux ABI compatibility. Tests are cross-compiled with musl libc for RISC-V 64-bit and run on the Rux kernel via QEMU.

**Location**: `userspace/linux-ltp/`

---

## Compilation Statistics

With musl libc cross-compilation:

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

The 101% rate indicates that some tests compile successfully despite not being in the expected list, demonstrating broad compatibility.

---

## Building

```bash
cd userspace/linux-ltp
./build.sh
```

Requirements:
- `riscv64-linux-gnu-gcc` — RISC-V cross-compiler
- musl libc SDK (built via `make sdk` in project root)

### Adding to Rootfs

After building, run `make rootfs` to include the tests in the rootfs image. Tests will be installed at `/test/linux-ltp/`.

---

## Running

In the Rux shell:

```bash
# Run quick test suite (essential tests only)
/test/linux-ltp/run_quick.sh

# Run syscall tests
/test/linux-ltp/run_syscalls.sh

# Run full LTP suite
/test/linux-ltp/run_ltp.sh
```

---

## Known Limitations

Some tests cannot compile with musl due to glibc-specific structures:

- `fmtmsg` — requires `addseverity()` (glibc extension)
- `ioctl` — requires `struct termio` (glibc specific)
- `timer_create` — requires `struct sigevent._sigev_un` (glibc internal)
- `statx` — requires `stx_mnt_id` field (newer kernel)
- `rt_tgsigqueueinfo` — requires `siginfo_t._sifields` (glibc internal)

The controllers category has a lower compilation rate (51%) due to cgroup-specific kernel interfaces not yet implemented in Rux.

---

## Test Categories

### Syscall Tests (1,378)

Covers 300+ syscall categories including:

- File operations: open, read, write, close, lseek, fstat, mmap
- Process management: fork, execve, wait, clone, exit, getpid
- IPC: pipe, socket, epoll, poll, eventfd, signalfd
- Signal handling: kill, sigaction, sigprocmask, sigaltstack
- Memory: brk, mprotect, munmap, mremap
- Network: socket, bind, listen, accept, connect, send, recv
- Scheduling: sched_yield, sched_setaffinity, sched_getaffinity

### Memory Tests (108)

Page fault handling, memory mapping, shared memory, huge pages, memory protection.

### Containers (46)

Namespace isolation, user namespaces, PID namespaces, network namespaces, mount namespaces.

### Controllers (20)

cgroup v1/v2 memory, CPU, blkio, devices, freezer controllers.

### Filesystem Tests (29)

ext4, tmpfs, procfs, sysfs, fuse stress tests and correctness verification.

### Security Tests (24)

Capability bounding, seccomp, SELinux, file permissions, access control lists.

### Scheduler Tests (23)

CFS scheduler, real-time scheduling, CPU affinity, priority inheritance, load balancing.

### IO Tests (19)

Direct I/O, async I/O, splice, sendfile, epoll edge-triggered, io_uring (if supported).

---

## Related Documents

- [Kernel Unit Test Report](unit-test-report.md)
- [Development Workflow](../development/development.md)
- [Design Documents](../architecture/design.md)
- [Roadmap](../progress/roadmap.md)

---

## Changelog

- **2026-03-30**: Split from unified testing.md into standalone report
- **2026-03-15**: Added Linux LTP test suite
  - Integrated official LTP test suite (1,838 tests)
  - Built with musl libc cross-compilation
  - 101% compilation success rate
