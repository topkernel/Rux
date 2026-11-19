#!/bin/bash
# build-rootfs.sh — build an ext4 rootfs carrying Geekbench for the
# x86_64 rig, mirroring the lmbench protocol (userspace/lmbench):
# a static musl runner at /test/geekbench_run is the PID-1 init and the
# suite lives under /test/geekbench/.
#
# Geekbench 5.5.1 (not 6.x): Geekbench 6.4.0 runs its workloads but
# exits 255 at the result-upload step without a Pro license and never
# prints scores; 5.5.1 "Tryout" prints the full score summary for free.
#
# The image is a private reflink copy of the shared x86_64 Ubuntu
# rootfs (glibc userland — the Geekbench binaries are dynamically
# linked against glibc). The Geekbench payload is ~130 MB unpacked, so
# the copy is grown by +1G (truncate + resize2fs) before injection.
#
# Usage: build-rootfs.sh <out.img> [base.img]
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${1:?usage: build-rootfs.sh <out.img> [base.img]}"
BASE="${2:-/home/william/workspace/Rux/work/ubuntu-x86.img}"
TC="/home/william/rux-agents/code-review-perf/toolchain/x86_64-rux-linux-musl/bin/musl-gcc"
TARBALL="$HERE/Geekbench-5.5.1-Linux.tar.gz"
GROW=1G

[ -f "$BASE" ] || { echo "base image missing: $BASE" >&2; exit 1; }
[ -x "$TC" ] || { echo "musl-gcc missing: $TC" >&2; exit 1; }
[ -f "$TARBALL" ] || { echo "tarball missing: $TARBALL" >&2; exit 1; }

# 1. Private copy (reflink when the filesystem supports it).
cp --reflink=auto "$BASE" "$OUT"

# 2. Unpack Geekbench (kept out of git: only the tarball is committed).
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
tar xzf "$TARBALL" -C "$STAGE"
GB="$STAGE/Geekbench-5.5.1-Linux"
for f in geekbench5 geekbench_x86_64 geekbench.plar; do
    [ -f "$GB/$f" ] || { echo "payload missing: $GB/$f" >&2; exit 1; }
done

# 3. Build the PID-1 runner (static musl).
"$TC" -static -O2 -o "$STAGE/geekbench_run" "$HERE/geekbench_run.c"

# 4. Grow the copy so the payload fits, then inject via debugfs.
truncate -s +$GROW "$OUT"
e2fsck -fy "$OUT" > /dev/null 2>&1 || true
resize2fs "$OUT" > /dev/null 2>&1

# 4a. Optional result-capture wiring. Geekbench only reveals scores in
# the HTTPS upload payload (tryout mode prints nothing locally and the
# browser result page is bot-guarded), so the rig redirects the upload
# to a local capture server: with GB_CAPTURE_CA set to a CA cert file,
# the guest gets /etc/ssl/certs/ca-certificates.crt = that CA and an
# /etc/hosts entry pinning browser.geekbench.com to 10.0.2.2 (the
# QEMU user-network alias for the host, where gb-capture-server.py
# listens on :443).
if [ -n "${GB_CAPTURE_CA:-}" ]; then
    [ -f "$GB_CAPTURE_CA" ] || { echo "GB_CAPTURE_CA missing: $GB_CAPTURE_CA" >&2; exit 1; }
    printf '127.0.0.1 localhost\n10.0.2.2 browser.geekbench.com\n' > "$STAGE/hosts"
    cp "$GB_CAPTURE_CA" "$STAGE/ca-certificates.crt"
fi

CMDS="$STAGE/cmds"
{
    echo "mkdir /test"
    echo "mkdir /test/geekbench"
    echo "write $GB/geekbench5 /test/geekbench/geekbench5"
    echo "sif /test/geekbench/geekbench5 mode 0100755"
    echo "write $GB/geekbench_x86_64 /test/geekbench/geekbench_x86_64"
    echo "sif /test/geekbench/geekbench_x86_64 mode 0100755"
    echo "write $GB/geekbench.plar /test/geekbench/geekbench.plar"
    echo "sif /test/geekbench/geekbench.plar mode 0100644"
    echo "write $STAGE/geekbench_run /test/geekbench_run"
    echo "sif /test/geekbench_run mode 0100755"
    if [ -n "${GB_CAPTURE_CA:-}" ]; then
        echo "mkdir /etc/ssl"
        echo "mkdir /etc/ssl/certs"
        echo "write $STAGE/ca-certificates.crt /etc/ssl/certs/ca-certificates.crt"
        echo "sif /etc/ssl/certs/ca-certificates.crt mode 0100644"
        echo "rm /etc/hosts"
        echo "write $STAGE/hosts /etc/hosts"
        echo "sif /etc/hosts mode 0100644"
    fi
} > "$CMDS"
debugfs -w -f "$CMDS" "$OUT" > /dev/null 2>&1 || true

# 5. Verify the payload landed.
for f in geekbench5 geekbench_x86_64 geekbench.plar; do
    sz_src="$(stat -c %s "$GB/$f")"
    sz_img="$(debugfs -R "stat /test/geekbench/$f" "$OUT" 2>/dev/null | grep -o 'Size: [0-9]*' | head -1 | cut -d' ' -f2)"
    [ "$sz_src" = "$sz_img" ] || { echo "inject verify FAILED for $f ($sz_src vs $sz_img)" >&2; exit 1; }
done

echo "build-rootfs: $OUT ready (init=/test/geekbench_run)"
