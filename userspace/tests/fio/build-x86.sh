#!/bin/bash
# build-x86.sh — static-musl fio for the qa-suites lane (x86_64 guest).
#
# Source of truth: fio-3.36.tar.gz (upstream GitHub tag archive, committed
# unmodified). All optional dependencies are DISABLED so only the musl
# toolchain links in (the host's glibc libcrypto/zstd must never leak
# into the guest binary). Engines available: sync/psync/vsync/pwrite/
# splice/null/net (io_uring/libaio are kernel-side future work).
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

mkdir -p "$SCRATCH" "$OUT"
rm -rf "$SCRATCH/fio-fio-3.36"
tar xzf "$DIR/fio-3.36.tar.gz" -C "$SCRATCH"
cd "$SCRATCH/fio-fio-3.36"
CC="$MUSL_CC" ./configure --disable-native --disable-zlib --disable-libzstd \
    --disable-libaio --disable-libnbd --disable-libcufile --disable-cuda \
    --disable-http --disable-nfs --disable-libiscsi --disable-libxnvme \
    --disable-arm64-crc --disable-numa --disable-libblkio > "$OUT/configure.log" 2>&1
make -j"$(nproc)" LDFLAGS=-static > "$OUT/build.log" 2>&1
cp fio "$OUT/fio"
file "$OUT/fio" | sed 's/, BuildID.*//'
