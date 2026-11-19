#!/bin/bash
# Rux OS - LTP Test Suite Build Script (x86_64, native musl)
#
# Native build on the x86_64 host via the musl-gcc wrapper
# (toolchain/x86_64-rux-linux-musl/bin/musl-gcc, restored from its tracked
# tarball by scripts/extract-probes.sh). No cross toolchain and NO soft-fp
# overlay — the BUG-S005 zbb/zcb SIGILL discipline is riscv64-only.
#
# Mirrors userspace/linux-ltp/build.sh (riscv64) in configure options and
# musl-compat patches so the x86_64 binary set matches the riscv one.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
OUTPUT_DIR="${SCRIPT_DIR}/output-x86_64"
MUSL_DIR="${PROJECT_ROOT}/toolchain/x86_64-rux-linux-musl"
MUSL_CC="${MUSL_DIR}/bin/musl-gcc"
LTP_VERSION="20240524"
LTP_SRC_DIR="${SCRIPT_DIR}/ltp-full-${LTP_VERSION}"

RED='\033[0;31m'
GREEN='\032[0;32m'
info() { echo -e "[INFO] $1"; }
error() { echo -e "${RED}[ERROR]${NC} $1"; exit 1; }

if [ ! -x "$MUSL_CC" ]; then
    error "musl-gcc wrapper not found at $MUSL_CC. Run: make probes"
fi

export CC="$MUSL_CC"
export LD="$MUSL_CC"
export AR=ar
export RANLIB=ranlib
export STRIP=strip
export CROSS_COMPILE=""

# Static, freestanding-ish CFLAGS; the musl-gcc specs already inject
# -nostdinc + the musl include dir and the musl lib search path.
export CFLAGS="-O2 -U_FORTIFY_SOURCE -static"
export LDFLAGS="-static"

cd "$LTP_SRC_DIR"

if [ ! -f Makefile ] || [ ! -f .configured-x86_64 ]; then
    info "Configuring LTP for x86_64 musl (native)..."
    ./configure \
        --prefix="${OUTPUT_DIR}" \
        --without-numa \
        --without-aio \
        --without-selinux \
        --without-crypto \
        --without-tirpc \
        --without-keyutils \
        --without-libcap \
        --without-libacl \
        --without-open-posix-testsuite \
        --without-realtime-testsuite

    # musl compatibility patches (same set as the riscv64 build)
    sed -i 's/#define HAVE_SYS_PIDFD_H 1/\/\* #undef HAVE_SYS_PIDFD_H \*\//' \
        include/config.h
    sed -i 's/\/\* #undef HAVE_STRUCT_FANOTIFY_EVENT_INFO_FID_FSID___VAL \*\//#define HAVE_STRUCT_FANOTIFY_EVENT_INFO_FID_FSID___VAL 1/' \
        include/config.h
    sed -i 's/#define HAVE_STRUCT_FANOTIFY_EVENT_INFO_PIDFD 1/\/\* #undef HAVE_STRUCT_FANOTIFY_EVENT_INFO_PIDFD \*\//' \
        include/config.h
    sed -i 's/#define HAVE_STRUCT_FANOTIFY_EVENT_INFO_ERROR 1/\/\* #undef HAVE_STRUCT_FANOTIFY_EVENT_INFO_ERROR \*\//' \
        include/config.h
    touch .configured-x86_64
fi

info "Building LTP libraries..."
touch include/stamp-h1 2>/dev/null || true
make -C lib -j"$(nproc)" AUTOHEADER=true > /dev/null 2>&1 || true
make -C libs -j"$(nproc)" AUTOHEADER=true > /dev/null 2>&1 || true

LIB_PATHS="-L${MUSL_DIR}/lib -L${LTP_SRC_DIR}/lib -L${LTP_SRC_DIR}/libs/lib -L${LTP_SRC_DIR}/libs/libltp -L${LTP_SRC_DIR}/libs/libnewipc -L${LTP_SRC_DIR}/libs/libnuma_helper"
for libdir in ${LTP_SRC_DIR}/libs/*/; do
    LIB_PATHS="$LIB_PATHS -L$libdir"
done

# Second library-path set used by ALL test sweeps below (same list as the
# riscv64 build.sh's syscall LIB_PATHS): mbind/move_pages link -lltpnuma
# and -lkerntest from testcases/kernel/lib.
KERNEL_LIB_PATHS="$LIB_PATHS -L${LTP_SRC_DIR}/testcases/kernel/lib -L${LTP_SRC_DIR}/testcases/kernel/mem/lib -L${LTP_SRC_DIR}/testcases/kernel/mem/hugetlb/lib -L${LTP_SRC_DIR}/testcases/kernel/controllers/libcontrollers"

info "Building syscall tests (xargs -P)..."
find testcases/kernel/syscalls -mindepth 1 -maxdepth 1 -type d | \
    xargs -P "$(nproc)" -I '{}' make -C '{}' -j2 \
        AUTOHEADER=true \
        CC="$MUSL_CC" \
        ADD_CFLAGS="-static -O2 -U_FORTIFY_SOURCE" \
        LDFLAGS="-static $KERNEL_LIB_PATHS" > /dev/null 2>&1 || true

for subdir in ipc io mem containers controllers fs security sched connectors crypto device-drivers firmware hotplug input irq lib logging numa power_management pty sound tracing uevents watchqueue; do
    if [ -d "testcases/kernel/$subdir" ]; then
        make -C "testcases/kernel/$subdir" -j"$(nproc)" -k \
            AUTOHEADER=true \
            CC="$MUSL_CC" \
            ADD_CFLAGS="-static -O2 -U_FORTIFY_SOURCE" \
            LDFLAGS="-static $KERNEL_LIB_PATHS" > /dev/null 2>&1 || true
    fi
done

for dir in testcases/misc testcases/network testcases/cve testcases/commands testcases/lib; do
    info "Building $dir..."
    make -C "$dir" -j"$(nproc)" -k \
        AUTOHEADER=true \
        CC="$MUSL_CC" \
        ADD_CFLAGS="-static -O2 -U_FORTIFY_SOURCE" \
        LDFLAGS="-static $KERNEL_LIB_PATHS" > /dev/null 2>&1 || true
done

info "Installing..."
mkdir -p "${OUTPUT_DIR}/testcases/bin" "${OUTPUT_DIR}/runtest" "${OUTPUT_DIR}/scenario_groups"
find testcases -type f -executable 2>/dev/null | while read -r f; do
    if file "$f" | grep -q "ELF.*executable"; then
        cp "$f" "${OUTPUT_DIR}/testcases/bin/" 2>/dev/null || true
    fi
done
cp -r runtest/* "${OUTPUT_DIR}/runtest/" 2>/dev/null || true
cp -r scenario_groups/* "${OUTPUT_DIR}/scenario_groups/" 2>/dev/null || true

info "Stripping binaries..."
find "${OUTPUT_DIR}/testcases/bin" -type f -exec strip {} \; 2>/dev/null || true

FINAL_COUNT=$(find "${OUTPUT_DIR}/testcases/bin" -type f | wc -l)
info "Installed $FINAL_COUNT test binaries to ${OUTPUT_DIR}"
