#!/bin/bash
# extract-lmbench.sh — unpack the lmbench benchmark suite into
# userspace/lmbench/src/. The tarball is tracked; the extracted tree is
# NOT (user directive 2026-10-31: benchmark sources stay out of git).
#
# Idempotent: skips when src/lmbench-master/src/lat_syscall.c exists.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
TARBALL="$HERE/lmbench-3.0-a9.tgz"
SRC="$HERE/src"

if [ -f "$SRC/lmbench-master/src/lat_syscall.c" ]; then
    echo "extract-lmbench: already extracted"
    exit 0
fi
[ -f "$TARBALL" ] || { echo "missing $TARBALL" >&2; exit 1; }

mkdir -p "$SRC"
tar -xzf "$TARBALL" -C "$SRC"
echo "extract-lmbench: unpacked into $SRC/lmbench-master"
