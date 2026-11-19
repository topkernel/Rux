# LTP Test Suite for Rux OS

This directory contains scripts to build the official LTP (Linux Test Project)
test suite for Rux OS kernel compatibility testing using musl libc.

## Building

```bash
# Build LTP tests
./build.sh

# Clean build artifacts (keeps tarball)
./build.sh clean
```

## Requirements

- `riscv64-linux-gnu-gcc` - RISC-V cross-compiler
- musl libc SDK (built via `make sdk` in project root)

## Output

After building, the output directory contains:

```
output/
├── run_ltp.sh      - Full test runner
├── run_quick.sh    - Quick test runner (essential tests only)
├── run_syscalls.sh - Syscall tests runner
├── testcases/      - LTP test binaries
│   └── bin/        - Individual test executables
└── ...
```

## Compilation Statistics

With musl libc cross-compilation, we achieve:

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

## Known Limitations

Some tests cannot compile with musl due to glibc-specific structures:

- `fmtmsg` - requires `addseverity()` (glibc extension)
- `ioctl` - requires `struct termio` (glibc specific)
- `timer_create` - requires `struct sigevent._sigev_un` (glibc internal)
- `statx` - requires `stx_mnt_id` field (newer kernel)
- `rt_tgsigqueueinfo` - requires `siginfo_t._sifields` (glibc internal)

## Adding to Rootfs

After building, run `make rootfs` to include the tests in the rootfs image.
Tests will be installed at `/test/linux-ltp/`.

## Running Tests

In Rux OS shell:

```bash
# Run quick test suite
/test/linux-ltp/run_quick.sh

# Run syscall tests
/test/linux-ltp/run_syscalls.sh

# Run full LTP suite
/test/linux-ltp/run_ltp.sh
```

## LTP Version

Current version: 20240524

For more information about LTP, see: https://github.com/linux-test-project/ltp
