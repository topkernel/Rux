# Toybox for Rux OS

This directory contains the build script for cross-compiling [Toybox](https://landley.net/toybox/) using musl libc for Rux OS.

## About Toybox

Toybox is a BSD-licensed replacement for BusyBox, providing 200+ standard Linux command line tools in a single binary. It is maintained by Rob Landley.

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
make toybox
```

Or directly:
```bash
cd userspace/toybox
./build-toybox.sh
```

## Build Process

1. Downloads toybox 0.8.13 source tarball (if not present)
2. Extracts to `toybox/` directory
3. Configures with `defconfig`
4. Disables commands requiring crypt library (su, login, mkpasswd)
5. Enables shell (toysh) command
6. Cross-compiles with musl libc

## Output

- `toybox/toybox` - Static RISC-V 64-bit binary (~850KB)

## Available Commands

Run `toybox --list` to see all 200+ available commands. Key categories:

- **File operations**: ls, cp, mv, rm, cat, head, tail, etc.
- **Text processing**: grep, sed, awk, sort, uniq, etc.
- **Shell**: sh (toysh)
- **System**: ps, top, mount, umount, etc.
- **Network**: wget, ping, ifconfig, etc.

## Disabled Commands

The following commands are disabled because they require the crypt library:
- `su` - Switch user
- `login` - User login
- `mkpasswd` - Password hashing

## Notes

- Source code is not modified, only configuration changes via sed
- The toybox source directory is gitignored and regenerated on each build
- To clean, simply delete the `toybox/` directory
