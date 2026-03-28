#!/bin/bash
# Rux OS - mrsh Build Script
#
# Cross-compile mrsh (minimal POSIX shell) using musl libc,
# generating statically linked RISC-V 64-bit binary.
# Links against troglobit/editline for line editing, history, and tab completion.

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

# ==================== Build troglobit/editline ====================

# Create wrapper headers for mrsh's HAVE_EDITLINE path
# troglobit/editline provides <editline.h>, but mrsh expects <editline/readline.h> + <histedit.h>
mkdir -p "${SCRIPT_DIR}/include/editline"
cat > "${SCRIPT_DIR}/include/editline/readline.h" << 'EOF'
#include "editline.h"
EOF
cat > "${SCRIPT_DIR}/include/histedit.h" << 'EOF'
/* empty stub - mrsh includes this with HAVE_EDITLINE but doesn't use anything from it */
EOF

EDITLINE_VER="1.17.1"
EDITLINE_TARBALL="editline-${EDITLINE_VER}.tar.xz"
EDITLINE_URL="https://ftp.troglobit.com/editline/${EDITLINE_TARBALL}"
EDITLINE_BUILD="${SCRIPT_DIR}/editline-build"

if [ ! -f "${SCRIPT_DIR}/${EDITLINE_TARBALL}" ]; then
    echo "Downloading troglobit/editline ${EDITLINE_VER}..."
    wget -q "$EDITLINE_URL" -O "${SCRIPT_DIR}/${EDITLINE_TARBALL}"
fi

echo "Building troglobit/editline ${EDITLINE_VER}..."
rm -rf "$EDITLINE_BUILD"
mkdir -p "$EDITLINE_BUILD"
tar -xf "${SCRIPT_DIR}/${EDITLINE_TARBALL}" -C "$EDITLINE_BUILD" --strip-components=1

cd "$EDITLINE_BUILD"
./configure \
    --host=riscv64-linux-gnu \
    --prefix="$EDITLINE_BUILD/install" \
    --disable-termcap \
    --enable-static \
    --disable-shared \
    CFLAGS="-static -nostdinc -fno-stack-protector -isystem ${MUSL_DIR}/include -isystem /usr/riscv64-linux-gnu/include -isystem /usr/include" \
    LDFLAGS="-static -nostdlib ${MUSL_DIR}/lib/crt1.o ${MUSL_DIR}/lib/crti.o -L${MUSL_DIR}/lib -lc -lgcc ${MUSL_DIR}/lib/crtn.o"

# Patch config.h: force HAVE_TCGETATTR since musl has tcgetattr() but
# configure couldn't detect it due to -nostdinc. Also undef HAVE_TERMIO_H
# since musl's termio.h has no usable struct termio, and define HAVE_STRDUP
# to avoid conflicting with musl's strdup.
sed -i 's/\/\* #undef HAVE_TCGETATTR \*\//#define HAVE_TCGETATTR 1/' "$EDITLINE_BUILD/config.h"
sed -i 's/#define HAVE_TERMIO_H 1/\/\* #undef HAVE_TERMIO_H \*\//' "$EDITLINE_BUILD/config.h"
sed -i 's/\/\* #undef HAVE_STRDUP \*\//#define HAVE_STRDUP 1/' "$EDITLINE_BUILD/config.h"
sed -i 's/\/\* #undef HAVE_STRCHR \*\//#define HAVE_STRCHR 1/' "$EDITLINE_BUILD/config.h"
sed -i 's/\/\* #undef HAVE_STRRCHR \*\//#define HAVE_STRRCHR 1/' "$EDITLINE_BUILD/config.h"
sed -i 's/\/\* #undef HAVE_PERROR \*\//#define HAVE_PERROR 1/' "$EDITLINE_BUILD/config.h"

make -j$(nproc) -C src
make -C src install
# Skip examples (testit.c has conflicting perror declaration with musl)
mkdir -p "$EDITLINE_BUILD/install/include"
mkdir -p "$EDITLINE_BUILD/install/lib"
cp "$EDITLINE_BUILD/include/editline.h" "$EDITLINE_BUILD/install/include/"
cp "$EDITLINE_BUILD/src/.libs/libeditline.a" "$EDITLINE_BUILD/install/lib/"

EDITLINE_INSTALL="$EDITLINE_BUILD/install"
echo "editline built: ${EDITLINE_INSTALL}/lib/libeditline.a"
echo ""

# ==================== Extract mrsh source ====================

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

# ==================== Build mrsh ====================

cd "$MRSH_DIR"

# Set cross-compile environment variables - using musl libc
# Note: -fno-stack-protector is needed because musl doesn't provide __stack_chk_guard
export CC=riscv64-linux-gnu-gcc
export CFLAGS="-static -nostdinc -fno-stack-protector -isystem ${MUSL_DIR}/include -isystem /usr/riscv64-linux-gnu/include -isystem /usr/include -DHAVE_EDITLINE -I${SCRIPT_DIR}/include -I${EDITLINE_INSTALL}/include"
export PKG_CONFIG=""

echo ""
echo "Configuring mrsh (with editline, static)..."
./configure --with-readline --static

# Patch config.mk to fix LDFLAGS and LIBS for musl static linking
# mrsh's configure adds its own LDFLAGS (soname, version-script, etc.) and clears LIBS
# We need to override them after configure runs
echo "Patching config.mk for musl static linking..."
CONFIG_MK=".build/config.mk"

# Replace LDFLAGS line: remove -nostdlib, keep -static and CRT objects
# The mrsh Makefile link line is: $(CC) -o $@ $(LDFLAGS) $(objects) -L$(OUTDIR) -lmrsh $(LIBS)
# We want: $(CC) -static crt1.o crti.o [objects] -lmrsh -lc -lgcc crtn.o -leditline
# Note: LDFLAGS has continuation lines (\), use awk to replace the whole block
awk -v musl="$MUSL_DIR" -v editline="$EDITLINE_INSTALL" '
    /^LDFLAGS=/ { skip=1; print "LDFLAGS=-static -nostdlib " musl "/lib/crt1.o " musl "/lib/crti.o"; next }
    /^LIBS=/ { skip=0; print "LIBS=-L" editline "/lib -leditline -L" musl "/lib -lc -lgcc " musl "/lib/crtn.o"; next }
    !skip { print }
' "$CONFIG_MK" > "$CONFIG_MK.tmp" && mv "$CONFIG_MK.tmp" "$CONFIG_MK"

# Switch frontend from basic.c to readline.c (configure didn't detect editline via pkg-config)
sed -i 's|frontend/basic\.o|frontend/readline.o|g' "$CONFIG_MK"
sed -i 's|frontend/basic\.c|frontend/readline.c|g' "$CONFIG_MK"

echo ""
echo "Building mrsh..."
# Clean old objects to force rebuild with new frontend
make -C "$MRSH_DIR" clean 2>/dev/null || true
make -j$(nproc) mrsh

# Verify build result
if [ -f "$MRSH_DIR/mrsh" ]; then
    echo ""
    echo "========================================"
    echo "mrsh built successfully (with editline)!"
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
