# Toolchain for Rux OS

This directory contains the musl libc toolchain for cross-compiling Rux OS
user programs.

## Building

```bash
# Build musl libc
./build-musl.sh

# Clean build artifacts
./build-musl.sh clean
```

## Requirements

- `riscv64-linux-gnu-gcc` - RISC-V cross-compiler
  ```bash
  apt install gcc-riscv64-linux-gnu
  ```

## Output

After building, the output directory contains:

```
riscv64-rux-linux-musl/
├── include/        - C header files (musl + kernel headers)
│   ├── rux/        - Rux specific headers
│   ├── linux/      - Linux kernel headers
│   ├── asm/        - Architecture-specific headers
│   └── ...         - Standard C library headers
└── lib/            - Static libraries
    ├── libc.a      - musl C library
    ├── crt1.o      - C runtime startup
    ├── crti.o      - Init code
    └── crtn.o      - End code
```

## Usage

Compile programs with musl libc:

```bash
riscv64-linux-gnu-gcc -static -nostdlib \
    -I toolchain/riscv64-rux-linux-musl/include \
    -L toolchain/riscv64-rux-linux-musl/lib \
    -o program program.c \
    toolchain/riscv64-rux-linux-musl/lib/crt1.o \
    toolchain/riscv64-rux-linux-musl/lib/libc.a \
    -lgcc
```

Or use CPPFLAGS/LDFLAGS:

```bash
export CPPFLAGS="-nostdinc -isystem toolchain/riscv64-rux-linux-musl/include"
export LDFLAGS="-static -L toolchain/riscv64-rux-linux-musl/lib"
riscv64-linux-gnu-gcc $CPPFLAGS $LDFLAGS -o program program.c
```

## musl Version

Current version: 1.2.5

For more information about musl, see: https://musl.libc.org/
