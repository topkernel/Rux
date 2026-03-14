#!/bin/bash
# Rux OS - Toybox Build Script
#
# Cross-compile toybox using musl libc, generating statically linked RISC-V 64-bit binary

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
TOYBOX_DIR="${SCRIPT_DIR}/toybox"
TOYBOX_VERSION="0.8.13"
MUSL_DIR="${PROJECT_ROOT}/toolchain/riscv64-rux-linux-musl"

echo "========================================"
echo "Rux OS - Toybox Build Script (musl libc)"
echo "========================================"
echo "TOYBOX_VERSION: ${TOYBOX_VERSION}"
echo "TOYBOX_DIR: ${TOYBOX_DIR}"
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

# Extract toybox source from local tarball
if [ ! -d "$TOYBOX_DIR" ]; then
    echo "Extracting toybox ${TOYBOX_VERSION}..."
    cd "$SCRIPT_DIR"

    TARBALL="toybox-${TOYBOX_VERSION}.tar.gz"
    if [ ! -f "$TARBALL" ]; then
        echo "Error: toybox tarball not found: $TARBALL"
        echo "Please ensure the tarball is present in $SCRIPT_DIR"
        exit 1
    fi

    tar -xzf "$TARBALL"
    mv "toybox-${TOYBOX_VERSION}" toybox
    echo "Toybox source extracted"
else
    echo "Toybox source already exists at $TOYBOX_DIR"
fi

# Build toybox
cd "$TOYBOX_DIR"

# Set cross-compile environment variables - using musl libc
# Include musl headers and system linux/asm headers
export CC=riscv64-linux-gnu-gcc
export CFLAGS="-static -nostdinc -isystem ${MUSL_DIR}/include -isystem /usr/riscv64-linux-gnu/include -isystem /usr/include"
export LDFLAGS="-static -nostdlib -L${MUSL_DIR}/lib ${MUSL_DIR}/lib/crt1.o ${MUSL_DIR}/lib/crti.o -lgcc ${MUSL_DIR}/lib/crtn.o -lc -lgcc"

echo ""
echo "Configuring toybox..."
make distclean 2>/dev/null || true
make defconfig

# Disable commands that require crypt library (su, login, mkpasswd)
echo "Disabling commands that require crypt library..."
# Use standard Linux kernel config format, toybox kconfig handles it correctly
sed -i 's/CONFIG_SU=y/# CONFIG_SU is not set/' .config
sed -i 's/CONFIG_LOGIN=y/# CONFIG_LOGIN is not set/' .config
sed -i 's/CONFIG_MKPASSWD=y/# CONFIG_MKPASSWD is not set/' .config

# Enable shell command
echo "Enabling sh (toysh) command..."
sed -i 's/# CONFIG_SH is not set/CONFIG_SH=y/' .config

# Regenerate configuration
./generated/unstripped/kconfig -s .config 2>/dev/null || true

# Fix toybox kconfig bug: CFG_XXX=n format doesn't generate USE_XXX macro
# Need to manually change "=n" to "= 0" and add USE_XXX(...) macro
fix_config_h() {
    local cmd=$1
    if grep -q "#define CFG_${cmd} n" generated/config.h 2>/dev/null; then
        sed -i "s/#define CFG_${cmd} n/#define CFG_${cmd} 0\n#define USE_${cmd}(...)\n#define SKIP_${cmd}(...) __VA_ARGS__/" generated/config.h
    fi
}

fix_config_h "SU"
fix_config_h "LOGIN"
fix_config_h "MKPASSWD"

# If SH is still disabled, force enable it
if grep -q "#define CFG_SH 0" generated/config.h 2>/dev/null; then
    echo "Force enabling SH in config.h..."
    sed -i 's/#define CFG_SH 0/#define CFG_SH 1/' generated/config.h
    sed -i 's/#define USE_SH(...)/#define USE_SH(...) __VA_ARGS__/' generated/config.h
    sed -i 's/#define SKIP_SH(...)/#define SKIP_SH(...)/' generated/config.h
fi

echo ""
echo "Building toybox (this may take a few minutes)..."
make -j$(nproc)

# Verify build result
if [ -f "$TOYBOX_DIR/toybox" ]; then
    echo ""
    echo "========================================"
    echo "Toybox built successfully!"
    echo "========================================"
    ls -la "$TOYBOX_DIR/toybox"
    file "$TOYBOX_DIR/toybox"
    echo ""
    echo "Binary size: $(du -h "$TOYBOX_DIR/toybox" | cut -f1)"
    echo "Output: $TOYBOX_DIR/toybox"
    echo ""
    echo "Note: su, login, mkpasswd commands are disabled (require crypt library)"
else
    echo "Error: toybox build failed"
    exit 1
fi
