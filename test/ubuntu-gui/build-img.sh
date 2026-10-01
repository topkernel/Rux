#!/bin/bash
# build-img.sh — assemble the Ubuntu GUI rootfs and the ext4 boot image.
#
# Copies the shared gate rootfs (never modified in place), cross-compiles the
# udesk session + init, installs them, and packs work/ubuntu-gui.img.
# Rux ext4 requirements: 4K blocks, no metadata_csum.
set -euo pipefail

WT="$(cd "$(dirname "$0")/../.." && pwd)"
SRC="${SRC:-/home/william/rux-gate/ubuntu/rootfs}"
WORK="$WT/work"
ROOT="$WORK/rootfs"
IMG="$WORK/ubuntu-gui.img"
CC="${CC:-riscv64-linux-gnu-gcc}"

command -v "$CC" >/dev/null || { echo "missing $CC" >&2; exit 1; }

rm -rf "$ROOT"
mkdir -p "$ROOT" "$WORK"
cp -a "$SRC/." "$ROOT/"

$CC -static -O2 -Wall -Wextra -o "$ROOT/usr/bin/udesk" "$WT/test/ubuntu-gui/udesk.c"
$CC -static -O2 -Wall -Wextra -o "$ROOT/sbin/udesk-init" "$WT/test/ubuntu-gui/init.c"
$CC -static -O2 -Wall -Wextra -o "$ROOT/sbin/shutdown" "$WT/test/ubuntu-gui/shutdown.c"

# preserve any pre-existing init exactly once
if [ ! -e "$ROOT/sbin/init.dist" ]; then
    mv "$ROOT/sbin/init" "$ROOT/sbin/init.dist"
fi
ln -sf /sbin/udesk-init "$ROOT/sbin/init"

rm -f "$IMG"
truncate -s 400M "$IMG"
mkfs.ext4 -F -b 4096 -O ^metadata_csum -d "$ROOT" -I 256 "$IMG" >/dev/null

echo "image ready: $IMG ($(du -h "$IMG" | cut -f1))"
file "$IMG"
