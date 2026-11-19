#!/bin/bash
# fetch-ubuntu-rootfs.sh — materialize the shared Ubuntu riscv64 base rootfs
# on ANY dev machine (no hand-carried artifacts). Idempotent.
#
# Downloads the official ubuntu-base 22.04 riscv64 tarball (a minimal
# glibc rootfs: dash, coreutils, apt, dpkg) and unpacks it to
#   work/ubuntu-base-rootfs/<version>/
# with a `current` symlink pointing at it. test/ubuntu-gui/build-img.sh
# uses this as its default SRC when /home/william/rux-gate is absent.
#
# Mirror order: TUNA (fast in CN) -> official cdimage. Override with
# UBUNTU_BASE_MIRRORS (space-separated URLs) / UBUNTU_BASE_VERSION.
set -euo pipefail

WT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$WT/work/ubuntu-base-rootfs"
VERSION="${UBUNTU_BASE_VERSION:-22.04.3}"
TARBALL="ubuntu-base-${VERSION}-base-riscv64.tar.gz"

if [ -d "$WORK/current/bin" ]; then
    echo "ubuntu base rootfs already at $WORK/current (delete to refetch)"
    exit 0
fi

MIRRORS=(
    "https://mirrors.tuna.tsinghua.edu.cn/ubuntu-cdimage/ubuntu-base/releases/22.04/release"
    "https://cdimage.ubuntu.com/ubuntu-base/releases/22.04/release"
)
if [ -n "${UBUNTU_BASE_MIRRORS:-}" ]; then
    # shellcheck disable=SC2206
    MIRRORS=($UBUNTU_BASE_MIRRORS)
fi

mkdir -p "$WORK"
cd "$WORK"
ok=""
for m in "${MIRRORS[@]}"; do
    echo "trying $m/$TARBALL ..."
    if curl -fL --retry 3 -o "$TARBALL.part" "$m/$TARBALL"; then
        mv "$TARBALL.part" "$TARBALL"; ok=1; break
    fi
done
[ -n "$ok" ] || { echo "all mirrors failed" >&2; exit 1; }

mkdir -p "$VERSION"
# ubuntu-base tarballs unpack with ./-relative paths
tar -xzf "$TARBALL" -C "$VERSION"
ln -sfn "$VERSION" current
rm -f "$TARBALL"
echo "rootfs ready: $WORK/current ($(du -sh "$WORK/current" | cut -f1))"
