# LTP Test Suite for Rux OS

This directory contains scripts to download, build, and package the official
LTP (Linux Test Project) test suite for Rux OS kernel compatibility testing.

## Building

```bash
# Build LTP tests
./build.sh

# Clean build artifacts
./build.sh clean
```

## Requirements

- `riscv64-linux-gnu-gcc` - RISC-V cross-compiler
- musl libc (built in `toolchain/riscv64-rux-linux-musl/`)

## Output

After building, the output directory contains:

```
output/
├── run_ltp.sh      - Full test runner
├── run_quick.sh    - Quick test runner (essential tests only)
├── testcases/      - LTP test binaries
│   └── bin/        - Individual test executables
└── ...
```

## Compilation Statistics

With musl libc cross-compilation, we achieve high coverage:

| Category | Count | Success Rate |
|----------|-------|--------------|
| **Total test binaries** | **1,826** | ~95% |
| Syscall tests | 1,367 | ~98% |
| Memory tests | 108 | ~99% |
| Containers | 46 | ~90% |
| Controllers | 39 | ~85% |
| Filesystem tests | 29 | ~90% |
| Security tests | 24 | ~80% |
| Scheduler tests | 23 | ~95% |
| IO tests | 19 | ~95% |
| Other tests | 171 | ~90% |

## Adding to Rootfs

After building, run `make rootfs` to include the tests in the rootfs image.
Tests will be installed at `/test/linux-ltp/`.

## Running Tests

In Rux OS shell:

```bash
# Run quick test suite
/test/linux-ltp/run_quick.sh

# Run full LTP suite
/test/linux-ltp/run_ltp.sh
```

## LTP Version

Current version: 20240524

For more information about LTP, see: https://github.com/linux-test-project/ltp
