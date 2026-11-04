#!/bin/bash
# build-x86.sh — static-musl fio for the qa-suites lane (x86_64 guest).
#
# Source of truth: fio-3.36.tar.gz + libaio_0.3.113.orig.tar.gz (both
# upstream archives, committed unmodified). All other optional
# dependencies are DISABLED so only the musl toolchain links in (the
# host's glibc libcrypto/zstd must never leak into the guest binary).
# Engines available: sync/psync/vsync/pwritev/splice/null/net/libaio
# (io_uring remains kernel-side future work). libaio is built from
# source WITHOUT compat-0_1.c: musl ships no <asm/errno.h> and that
# object only provides the pre-0.3 ABI symbols fio never touches.
#
# Output: run/fio-bin/fio
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$DIR/../../.." && pwd)"
SCRATCH="${SCRATCH:-$ROOT/run/fio-build}"
OUT="${OUT:-$ROOT/run/fio-bin}"
MUSL_CC="$ROOT/toolchain/x86_64-rux-linux-musl/bin/musl-gcc"

[ -x "$MUSL_CC" ] || {
    echo "error: $MUSL_CC missing — extract toolchain/x86_64-rux-linux-musl.tar.gz first" >&2
    exit 1
}

# ---- stage 0: kernel UAPI headers staged into a private prefix ----
# fio's os/os-linux.h includes <linux/unistd.h> <linux/fs.h> <scsi/sg.h>
# <asm/byteorder.h>; the musl toolchain ships no kernel UAPI headers and
# we must NOT add /usr/include wholesale (host glibc root headers —
# zlib.h etc. — would leak in and link host libc). Stage only the UAPI
# subdirectories from the host's linux-libc-dev (x86_64, matches the
# guest arch).
UAPI_PREFIX="$SCRATCH/linux-uapi"
rm -rf "$UAPI_PREFIX"
mkdir -p "$UAPI_PREFIX"
cp -r /usr/include/linux "$UAPI_PREFIX/linux"
cp -r /usr/include/x86_64-linux-gnu/asm "$UAPI_PREFIX/asm"
cp -r /usr/include/asm-generic "$UAPI_PREFIX/asm-generic"
mkdir -p "$UAPI_PREFIX/scsi"
cp /usr/include/scsi/sg.h "$UAPI_PREFIX/scsi/"

mkdir -p "$SCRATCH" "$OUT"

# ---- stage 1: libaio (static, musl; no compat-0_1.c — see header note) ----
LIBAIO_PREFIX="$SCRATCH/libaio-prefix"
rm -rf "$SCRATCH/libaio-0.3.113" "$LIBAIO_PREFIX"
tar xzf "$DIR/libaio_0.3.113.orig.tar.gz" -C "$SCRATCH"
make -C "$SCRATCH/libaio-0.3.113/src" CC="$MUSL_CC" ENABLE_SHARED=0 \
    libaio_srcs="io_queue_init.c io_queue_release.c io_queue_wait.c \
    io_queue_run.c io_getevents.c io_submit.c io_cancel.c io_setup.c \
    io_destroy.c io_pgetevents.c raw_syscall.c" libaio.a > "$OUT/libaio.log" 2>&1
mkdir -p "$LIBAIO_PREFIX/include" "$LIBAIO_PREFIX/lib"
cp "$SCRATCH/libaio-0.3.113/src/libaio.h" "$LIBAIO_PREFIX/include/"
cp "$SCRATCH/libaio-0.3.113/src/libaio.a" "$LIBAIO_PREFIX/lib/"

# ---- stage 2: fio (static, musl, libaio engine enabled) ----
# NOTE: fio 3.36's configure accepts ONLY these --disable-* flags (the
# round-4 script's zlib/zstd/libaio/nbd/cuda/nfs/iscsi/xnvme/arm64-crc
# ones are "Bad option" and make configure exit 1 without writing
# config-host.mak — that build only worked because configure had also
# been run by hand with valid flags). zlib/zstd are simply not found
# by musl-gcc and end up off; everything else unwanted needs no flag.
rm -rf "$SCRATCH/fio-fio-3.36"
tar xzf "$DIR/fio-3.36.tar.gz" -C "$SCRATCH"
cd "$SCRATCH/fio-fio-3.36"
CC="$MUSL_CC" CFLAGS="-I$LIBAIO_PREFIX/include -I$UAPI_PREFIX -O2" \
    LDFLAGS="-L$LIBAIO_PREFIX/lib" ./configure --disable-native \
    --disable-numa --disable-http > "$OUT/configure.log" 2>&1
grep -q "^CONFIG_LIBAIO=y" config-host.mak || {
    cat "$OUT/configure.log" >&2; echo "error: libaio engine not detected" >&2; exit 1; }
make -j"$(nproc)" LDFLAGS="-static -L$LIBAIO_PREFIX/lib" > "$OUT/build.log" 2>&1
cp fio "$OUT/fio"
file "$OUT/fio" | sed 's/, BuildID.*//'
