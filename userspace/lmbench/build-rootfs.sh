#!/bin/bash
# build-rootfs.sh — build an ext4 rootfs carrying the lmbench suite for
# the x86_64 rig, mirroring the LTP lane's shape (test/ltpscan): a
# static musl runner at /test/lmbench_run is the PID-1 init, the
# benchmarks live under /test/.
#
# The image is a private copy of the shared x86_64 Ubuntu rootfs (the
# same one the LTP x86 leg boots), so the guest environment — libc,
# shell, /proc, /sys — matches the LTP runs.
#
# Usage: build-rootfs.sh <out.img> [base.img]
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${1:?usage: build-rootfs.sh <out.img> [base.img]}"
BASE="${2:-/home/william/workspace/Rux/work/ubuntu-x86.img}"
TC="/home/william/rux-agents/code-review-perf/toolchain/x86_64-rux-linux-musl/bin/musl-gcc"

[ -f "$BASE" ] || { echo "base image missing: $BASE" >&2; exit 1; }
[ -x "$TC" ] || { echo "musl-gcc missing: $TC" >&2; exit 1; }

# 1. Private copy (reflink when the filesystem supports it).
cp --reflink=auto "$BASE" "$OUT"

# 2. Build the benchmarks + runner.
[ -d "$HERE/bin" ] || "$HERE/build-lmbench.sh"
"$TC" -static -O2 -o "$HERE/lmbench_run" "$HERE/lmbench_run.c"

# 3. Inject into the image via debugfs (no root, no loop mounts).
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
mkdir -p "$STAGE/test"
for b in "$HERE"/bin/*; do
    case "$b" in *.build.log) continue;; esac
    cp "$b" "$STAGE/test/"
done
cp "$HERE/lmbench_run" "$STAGE/lmbench_run"
printf '/* lmbench stat/open test file */\n' > "$STAGE/types.h"

CMDS="$STAGE/cmds"
{
    echo "mkdir /test"
    for f in "$STAGE"/test/*; do
        echo "write $f /test/$(basename "$f")"
    done
    echo "rm /test/lmbench_run"
    echo "write $STAGE/lmbench_run /test/lmbench_run"
    echo "sif /test/lmbench_run mode 0100755"
    # lmbench's lat_syscall stat/fstat/open cases stat this exact path
    # (lat_syscall.c: #define FNAME "/usr/include/linux/types.h"); the
    # Ubuntu rootfs ships it under the multiarch subdirectory, so the
    # guest needs the plain path to exist.
    echo "mkdir /usr/include/linux"
    echo "write $STAGE/types.h /usr/include/linux/types.h"
} > "$CMDS"
debugfs -w -f "$CMDS" "$OUT" > /dev/null 2>&1 || true

echo "build-rootfs: $OUT ready (init=/test/lmbench_run)"
