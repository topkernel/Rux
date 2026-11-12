#!/bin/bash
# setup-images.sh — ensure the project-local images/ tree exists and is
# populated. Other worktrees call this (or their boot scripts source it)
# to guarantee the images they need are available. Uses reflink copies
# where possible (zero extra disk on btrfs/XFS).
#
# Sources (in priority order):
#   1. images/ already present → no-op
#   2. /home/william/rux-images/ (the old central store)
#   3. /home/william/workspace/oh-robot/out/.../images/ (the build tree)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMGDIR="$ROOT/images"
OH_SRC_STORE="/home/william/rux-images/oh"
OH_BUILD="/home/william/workspace/oh-robot/out/x86_64_virt/packages/phone/images"

mkdir -p "$IMGDIR/oh"

need_copy() {
    local dst="$1" src="$2"
    [ -f "$dst" ] && return 0
    [ -f "$src" ] || return 1
    cp --reflink=auto "$src" "$dst" 2>/dev/null || cp "$src" "$dst"
    echo "  restored: $(basename $dst) ($(stat -c%s $dst) bytes)"
}

echo "Setting up images at $IMGDIR ..."

for img in ramdisk.img system.img vendor.img updater.img sys_prod.img chip_prod.img userdata.img eng_system.img; do
    if [ -f "$IMGDIR/oh/$img" ]; then
        continue
    elif [ -f "$OH_SRC_STORE/$img" ]; then
        need_copy "$IMGDIR/oh/$img" "$OH_SRC_STORE/$img"
    elif [ -f "$OH_BUILD/$img" ]; then
        need_copy "$IMGDIR/oh/$img" "$OH_BUILD/$img"
    else
        echo "  WARNING: $img not found in any source"
    fi
done

# LTP / benchmark images
if [ -d "/home/william/rux-images/testimages" ]; then
    mkdir -p "$IMGDIR/testimages"
    for img in /home/william/rux-images/testimages/*.img; do
        [ -f "$img" ] && need_copy "$IMGDIR/testimages/$(basename $img)" "$img"
    done
fi

echo "Done. $(find $IMGDIR -name '*.img' | wc -l) images available."
