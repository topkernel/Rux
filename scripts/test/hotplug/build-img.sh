#!/bin/bash
# build-img.sh — assemble the udev/hotplug verification image.
#
# Copies the shared gate rootfs (never modified in place), installs the
# cross-compiled busybox (mdev), the uevent netlink probe and the
# serial-driven init, then packs a fresh ext4 boot image.
# Rux ext4 requirements: 4K blocks, no metadata_csum.
set -euo pipefail

WT="$(cd "$(dirname "$0")/../.." && pwd)"
SRC="${SRC:-/home/william/rux-gate/ubuntu/rootfs}"
WORK="${WORK:-$WT/.udev}"
ROOT="$WORK/rootfs"
IMG="${IMG:-$WORK/hotplug.img}"
BUSYBOX="${BUSYBOX:-$WORK/busybox-1.36.1/busybox}"
CC="${CC:-riscv64-linux-gnu-gcc}"

command -v "$CC" >/dev/null || { echo "missing $CC" >&2; exit 1; }
[ -x "$BUSYBOX" ] || { echo "missing $BUSYBOX (build busybox first)" >&2; exit 1; }

rm -rf "$ROOT"
mkdir -p "$ROOT" "$WORK"
cp -a "$SRC/." "$ROOT/"

# probe (netlink uevent listener)
"$CC" -static -O2 -Wall -Wextra -o "$ROOT/bin/uevent_probe" "$WT/test/hotplug/uevent_probe.c"

# busybox + the applet symlinks we need (multi-call binary)
cp "$BUSYBOX" "$ROOT/bin/busybox"
for app in mdev sh stat grep ls cat rm sync sleep mkdir; do
    ln -sf /bin/busybox "$ROOT/bin/$app"
done

# init + hotplug checker
riscv64-linux-gnu-gcc -static -O2 -Wall -Wextra -o "$ROOT/sbin/hp-init" "$WT/test/hotplug/init.c"
chmod 755 "$ROOT/sbin/hp-init"
cp "$WT/test/hotplug/hpcheck.sh" "$ROOT/bin/hpcheck"
chmod 755 "$ROOT/bin/hpcheck"

# preserve the distro init exactly once, then take over PID 1
if [ ! -e "$ROOT/sbin/init.dist" ]; then
    mv "$ROOT/sbin/init" "$ROOT/sbin/init.dist"
fi
ln -sf /sbin/hp-init "$ROOT/sbin/init"

rm -f "$IMG"
truncate -s 400M "$IMG"
mkfs.ext4 -F -b 4096 -O ^metadata_csum -d "$ROOT" -I 256 "$IMG" >/dev/null

echo "image ready: $IMG ($(du -h "$IMG" | cut -f1))"
