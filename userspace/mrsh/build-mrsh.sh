#!/bin/bash
# Rux OS - mrsh Build Script
#
# Cross-compile mrsh (minimal POSIX shell) using musl libc,
# generating statically linked RISC-V 64-bit binary.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
MRSH_DIR="${SCRIPT_DIR}/mrsh"
MRSH_TARBALL="mrsh-master-4c81598.tar.gz"
MUSL_DIR="${PROJECT_ROOT}/toolchain/riscv64-rux-linux-musl"

echo "========================================"
echo "Rux OS - mrsh Build Script (musl libc)"
echo "========================================"
echo "MRSH_DIR: ${MRSH_DIR}"
echo "PROJECT_ROOT: ${PROJECT_ROOT}"
echo "MUSL_DIR: ${MUSL_DIR}"
echo ""

# Check cross-compiler toolchain
if ! command -v riscv64-linux-gnu-gcc &> /dev/null; then
    echo "Error: riscv64-linux-gnu-gcc not found"
    echo "Please install RISC-V cross-compiler toolchain"
    exit 1
fi

# Check musl directory
if [ ! -d "$MUSL_DIR/include" ]; then
    echo "Error: musl include directory not found at $MUSL_DIR/include"
    exit 1
fi

echo "Cross-compiler: $(which riscv64-linux-gnu-gcc)"
echo "GCC version: $(riscv64-linux-gnu-gcc --version | head -1)"
echo ""

# Extract mrsh source from local tarball
if [ ! -d "$MRSH_DIR" ]; then
    echo "Extracting mrsh..."
    cd "$SCRIPT_DIR"

    if [ ! -f "$MRSH_TARBALL" ]; then
        echo "Error: mrsh tarball not found: $MRSH_TARBALL"
        exit 1
    fi

    mkdir -p mrsh
    tar -xzf "$MRSH_TARBALL" -C mrsh
    echo "mrsh source extracted"
else
    echo "mrsh source already exists at $MRSH_DIR"
fi

# Build mrsh
cd "$MRSH_DIR"

# Set cross-compile environment variables - using musl libc
# Note: -fno-stack-protector is needed because musl doesn't provide __stack_chk_guard
export CC=riscv64-linux-gnu-gcc
export CFLAGS="-static -nostdinc -fno-stack-protector -isystem ${MUSL_DIR}/include -isystem /usr/riscv64-linux-gnu/include -isystem /usr/include"
export PKG_CONFIG=""

echo ""
echo "Configuring mrsh (without readline, static)..."
./configure --without-readline --static

# Patch config.mk to fix LDFLAGS and LIBS for musl static linking
# mrsh's configure adds its own LDFLAGS (soname, version-script, etc.) and clears LIBS
# We need to override them after configure runs
echo "Patching config.mk for musl static linking..."
CONFIG_MK=".build/config.mk"

# Replace LDFLAGS line: remove -nostdlib, keep -static and CRT objects
# The mrsh Makefile link line is: $(CC) -o $@ $(LDFLAGS) $(objects) -L$(OUTDIR) -lmrsh $(LIBS)
# We want: $(CC) -static crt1.o crti.o [objects] -lmrsh -lc -lgcc crtn.o
# Note: LDFLAGS has continuation lines (\), use awk to replace the whole block
awk -v musl="$MUSL_DIR" '
    /^LDFLAGS=/ { skip=1; print "LDFLAGS=-static -nostdlib " musl "/lib/crt1.o " musl "/lib/crti.o"; next }
    /^LIBS=/ { skip=0; print "LIBS=-L" musl "/lib -lc -lgcc " musl "/lib/crtn.o"; next }
    !skip { print }
' "$CONFIG_MK" > "$CONFIG_MK.tmp" && mv "$CONFIG_MK.tmp" "$CONFIG_MK"

echo ""
echo "Building mrsh..."
make -j$(nproc) mrsh

# Verify build result
if [ -f "$MRSH_DIR/mrsh" ]; then
    echo ""
    echo "========================================"
    echo "mrsh built successfully!"
    echo "========================================"
    ls -la "$MRSH_DIR/mrsh"
    file "$MRSH_DIR/mrsh"
    echo ""
    echo "Binary size: $(du -h "$MRSH_DIR/mrsh" | cut -f1)"
    echo "Output: $MRSH_DIR/mrsh"
else
    echo "Error: mrsh build failed"
    exit 1
fi
