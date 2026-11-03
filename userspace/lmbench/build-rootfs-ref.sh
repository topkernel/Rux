#!/bin/bash
# build-rootfs-ref.sh — build the ext4 rootfs for the reference-kernel
# column of the lmbench fair-rig comparison (run-refkvm.sh).
#
# Same shape as build-rootfs.sh (the Rux column's image): a copy of the
# shared x86_64 Ubuntu rootfs carrying the static musl benchmarks under
# /test plus the PID-1 lmbench_run. Two reference-column additions:
#
#   /test/init_ref   the PID-1 wrapper (init_ref.c) that mounts
#                    proc/sys/devtmpfs and execs lmbench_run — the
#                    reference kernel does not pre-mount /proc and the
#                    rootfs has no static /dev/console
#   /tmp/hello       the exact static binary the NATIVE reference run
#                    used for lat_proc's exec case (/tmp/hello on the
#                    host, 2026-11-01) — without it the case would
#                    measure a failing execve (the state the Rux
#                    column's image is in; see the fair-rig section of
#                    docs/test/lmbench-report.md)
#
# Usage: build-rootfs-ref.sh <out.img> [base.img] [hello.bin]
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${1:?usage: build-rootfs-ref.sh <out.img> [base.img] [hello.bin]}"
BASE="${2:-/home/william/workspace/Rux/work/ubuntu-x86.img}"
HELLO="${3:-/tmp/hello}"
TC="/home/william/rux-agents/code-review-perf/toolchain/x86_64-rux-linux-musl/bin/musl-gcc"

[ -f "$BASE" ] || { echo "base image missing: $BASE" >&2; exit 1; }
[ -x "$TC" ] || { echo "musl-gcc missing: $TC" >&2; exit 1; }
[ -f "$HELLO" ] || { echo "hello binary missing: $HELLO" >&2; exit 1; }

# 1-2. Base image with the benchmark suite (shared with the Rux
# column's builder): /test/* + /test/lmbench_run + the
# /usr/include/linux/types.h placeholder for lat_syscall stat/open.
"$HERE/build-rootfs.sh" "$OUT" "$BASE"

# 3. Reference-column additions.
"$TC" -static -O2 -o "$HERE/init_ref" "$HERE/init_ref.c"

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
cp "$HERE/init_ref" "$STAGE/init_ref"
cp "$HELLO" "$STAGE/hello"

{
    echo "rm /test/init_ref"
    echo "write $STAGE/init_ref /test/init_ref"
    echo "sif /test/init_ref mode 0100755"
    echo "write $STAGE/hello /tmp/hello"
    echo "sif /tmp/hello mode 0100755"
} | debugfs -w "$OUT" > /dev/null 2>&1 || true

echo "build-rootfs-ref: $OUT ready (init=/test/init_ref, /tmp/hello installed)"
