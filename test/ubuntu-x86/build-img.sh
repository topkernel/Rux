#!/bin/bash
# build-img.sh — assemble the Ubuntu amd64 rootfs and the ext4 boot image
# (x86 mirror of test/ubuntu-gui/build-img.sh).
#
# Downloads the Ubuntu 22.04 base amd64 rootfs (cached under
# work/downloads), natively compiles the same udesk session + init as the
# riscv64 image, installs them, and packs work/ubuntu-x86.img.
# Rux ext4 requirements: 4K blocks, no metadata_csum.
set -euo pipefail

WT="$(cd "$(dirname "$0")/../.." && pwd)"
DL="$WT/work/downloads"
WORK="$WT/work"
ROOT="$WORK/rootfs-x86"
IMG="$WORK/ubuntu-x86.img"
# Host is amd64 Linux: the stock gcc targets the guest directly.
CC="${CC:-gcc}"
SRC_TGZ="$DL/ubuntu-base-22.04-base-amd64.tar.gz"
SRC_URL="http://cdimage.ubuntu.com/ubuntu-base/releases/22.04/release/ubuntu-base-22.04-base-amd64.tar.gz"

command -v "$CC" >/dev/null || { echo "missing $CC" >&2; exit 1; }
command -v mkfs.ext4 >/dev/null || { echo "missing mkfs.ext4" >&2; exit 1; }

mkdir -p "$DL"

if [ ! -s "$SRC_TGZ" ]; then
    echo "downloading $SRC_URL"
    curl -fL --retry 3 -o "$SRC_TGZ" "$SRC_URL"
fi

rm -rf "$ROOT"
mkdir -p "$ROOT" "$WORK"
# ubuntu-base packs as a chroot tarball (paths relative to /, no leading ./)
tar -xzf "$SRC_TGZ" -C "$ROOT"

# Same session stack as the riscv64 image; the sources are plain POSIX C.
$CC -static -O2 -Wall -Wextra -o "$ROOT/usr/bin/udesk" "$WT/test/ubuntu-gui/udesk.c"
$CC -static -O2 -Wall -Wextra -o "$ROOT/sbin/udesk-init" "$WT/test/ubuntu-gui/init.c"
$CC -static -O2 -Wall -Wextra -o "$ROOT/sbin/shutdown" "$WT/test/ubuntu-gui/shutdown.c"
$CC -static -O2 -Wall -Wextra -o "$ROOT/usr/bin/swap-probe" "$WT/test/ubuntu-gui/swap-probe.c"

# ubuntu-base ships no /bin/true of its own in some revisions; the init
# warmup execs it, so make sure a /bin/true exists (coreutils provides it
# when present — install a tiny fallback only if missing).
if [ ! -e "$ROOT/bin/true" ] && [ ! -L "$ROOT/bin/true" ]; then
    $CC -static -O2 -o "$ROOT/bin/true" -x c - <<'EOF'
int main(void) { return 0; }
EOF
fi

# preserve any pre-existing init exactly once
if [ ! -e "$ROOT/sbin/init.dist" ]; then
    mv "$ROOT/sbin/init" "$ROOT/sbin/init.dist" 2>/dev/null || true
fi
ln -sf /sbin/udesk-init "$ROOT/sbin/init"

rm -f "$IMG"
truncate -s 400M "$IMG"
mkfs.ext4 -F -b 4096 -O ^metadata_csum -d "$ROOT" -I 256 "$IMG" >/dev/null

# Swap tail carve (Kernel.toml swap_size_mb=256): same layout as the riscv64
# image — filesystem sized at 400M, raw image grown to 700M so 300M of
# non-fs space at the end of the disk can back the kernel swap area. Do NOT
# resize2fs the filesystem into that tail.
truncate -s 700M "$IMG"

echo "image ready: $IMG ($(du -h "$IMG" | cut -f1))"
file "$IMG"
