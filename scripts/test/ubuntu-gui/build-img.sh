#!/bin/bash
# build-img.sh — assemble the Ubuntu GUI rootfs and the ext4 boot image.
#
# Copies the shared gate rootfs (never modified in place), cross-compiles the
# udesk session + init, installs them, and packs work/ubuntu-gui.img.
# Rux ext4 requirements: 4K blocks, no metadata_csum.
set -euo pipefail

WT="$(cd "$(dirname "$0")/../.." && pwd)"
# SRC priority: explicit override -> legacy shared gate rootfs ->
# auto-fetched ubuntu-base (scripts/fetch-ubuntu-rootfs.sh, works on any
# dev machine).
WORK="$WT/work"
ROOT="$WORK/rootfs"
IMG="$WORK/ubuntu-gui.img"
# Compiler: prefer our musl toolchain (march-pinned). The host cross-gcc
# statically links the HOST glibc — Ubuntu 25.10 glibc assumes the RVA23
# baseline (vector) and SIGILLs on QEMU rv64. musl avoids that.
MUSL="$WT/toolchain/riscv64-rux-linux-musl"
if [ -z "${CC:-}" ] && [ -f "$MUSL/lib/crt1.o" ]; then
    musl_cc() {
        riscv64-linux-gnu-gcc -static -march=rv64gc_zicsr -O2 -nostdinc -isystem /usr/lib/gcc-cross/riscv64-linux-gnu/13/include -isystem "$MUSL/include" -nostdlib "$MUSL/lib/crt1.o" -L"$MUSL/lib" -o "$1" "$2" -lc -lgcc
    }
    CC=musl_cc
else
    CC="${CC:-riscv64-linux-gnu-gcc}"
fi

command -v "$CC" >/dev/null || { echo "missing $CC" >&2; exit 1; }

if [ -z "${SRC:-}" ]; then
    if [ -d /home/william/rux-gate/ubuntu/rootfs ]; then
        SRC=/home/william/rux-gate/ubuntu/rootfs
    else
        "$WT/scripts/fetch-ubuntu-rootfs.sh"
        SRC="$WT/work/ubuntu-base-rootfs/current"
    fi
fi

rm -rf "$ROOT"
mkdir -p "$ROOT" "$WORK"
cp -a "$SRC/." "$ROOT/"

"$CC" "$ROOT/usr/bin/udesk" "$WT/test/ubuntu-gui/udesk.c"
"$CC" "$ROOT/sbin/udesk-init" "$WT/test/ubuntu-gui/init.c"
"$CC" "$ROOT/sbin/shutdown" "$WT/test/ubuntu-gui/shutdown.c"
# Swap gate probe: touches >2 GiB of anonymous memory and survives only
# when the swap tail carve is active (mm/swap.rs + vmscan reclaim).
"$CC" "$ROOT/usr/bin/swap-probe" "$WT/test/ubuntu-gui/swap-probe.c"

# preserve any pre-existing init exactly once (ubuntu-base ships none —
# it is a container rootfs; udesk-init becomes PID 1 directly)
if [ -e "$ROOT/sbin/init" ] && [ ! -e "$ROOT/sbin/init.dist" ]; then
    mv "$ROOT/sbin/init" "$ROOT/sbin/init.dist"
fi
ln -sf /sbin/udesk-init "$ROOT/sbin/init"

rm -f "$IMG"
truncate -s 400M "$IMG"
mkfs.ext4 -F -b 4096 -O ^metadata_csum -d "$ROOT" -I 256 "$IMG" >/dev/null

# Swap tail carve (Kernel.toml swap_size_mb=256): the ext4 filesystem is
# sized at 400M and the raw image is then grown to 700M, leaving 300M of
# non-fs space at the end of the disk for the kernel swap area. mm/swap.rs
# refuses to enable swap when the tail carve would overlap the filesystem,
# so the fs must NOT be grown to fill the image (no resize2fs here).
truncate -s 700M "$IMG"

echo "image ready: $IMG ($(du -h "$IMG" | cut -f1))"
file "$IMG"
