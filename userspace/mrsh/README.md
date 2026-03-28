# mrsh for Rux OS

This directory contains the build script for cross-compiling [mrsh](https://github.com/emersion/mrsh) using musl libc for Rux OS.

## About mrsh

mrsh is a minimal POSIX-compliant shell, licensed under the MIT license. It implements POSIX shell semantics strictly — no more, no less.

- **License**: MIT
- **Language**: C99
- **POSIX Compliance**: Strict POSIX shell, no bash extensions

## Source Code

The mrsh source tarball (`mrsh-master-4c81598.tar.gz`) is kept in this directory and tracked in git. The extracted source directory is gitignored and regenerated on each build.

The tarball is generated from commit `4c81598721bc5eeb28f9faa818b3102d0471b7f6` on the master branch.

## Building

### Prerequisites

1. RISC-V cross-compiler toolchain:
   ```bash
   sudo apt install gcc-riscv64-linux-gnu
   ```

2. musl libc SDK (build first):
   ```bash
   make sdk
   ```

### Build Commands

From project root:
```bash
make mrsh
```

Or directly:
```bash
cd userspace/mrsh
./build-mrsh.sh
```

## Build Process

1. Extracts mrsh from local tarball
2. Configures without readline (uses basic frontend)
3. Cross-compiles with musl libc as a static binary

## Output

- `mrsh/mrsh` - Static RISC-V 64-bit binary

## Usage in Rux OS

mrsh is installed as `/bin/sh` in the rootfs, replacing toybox's sh. It serves as the default system shell for running scripts (`sh -c "command"`).

The custom shell (`/bin/shell`) remains as the interactive shell.

## Notes

- Source code is not modified
- The extracted `mrsh/` directory is gitignored
- To clean, simply delete the `mrsh/` directory
