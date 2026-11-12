#!/bin/bash
# oh-link.sh — wire the OpenHarmony image set into the worktree (make oh-image).
#
# The OH emulator images live OUTSIDE git in the standing store (repo
# policy: real image bytes never enter a worktree). This script only
# creates symlinks and verifies presence — the boot itself is
# scripts/test/run-oh.sh (make oh-run), same shape as the Ubuntu flow.
#
# Guardrails (added after a swapped-argument incident corrupted the
# store itself): this script never writes inside the store, aborts if
# the link dir and the store overlap, and fails loudly on a dangling
# store entry instead of linking it.
#
# Usage: oh-link.sh [store-dir] [link-dir]
#   store-dir  default /home/william/rux-images/oh
#   link-dir   default run/oh-img
set -euo pipefail

STORE="${1:-/home/william/rux-images/oh}"
LINKDIR="${2:-run/oh-img}"

[ -d "$STORE" ] || {
    echo "ERROR: OH image store not found: $STORE" >&2
    echo "  Set OH_IMAGES=<dir> pointing at a directory with the OH" >&2
    echo "  x86_64_virt emulator images (ramdisk/updater/system/vendor/" >&2
    echo "  sys_prod/chip_prod/userdata .img)." >&2
    exit 1
}

# Guard: the link dir must not live inside the store, and the store
# must not live inside the link dir (either way a link could point at
# its own directory — the self-loop corruption class).
STORE_ABS="$(cd "$STORE" && pwd)"
mkdir -p "$LINKDIR"
LINK_ABS="$(cd "$LINKDIR" && pwd)"
case "$LINK_ABS/" in "$STORE_ABS"/*) echo "ERROR: link dir ($LINK_ABS) is inside the store ($STORE_ABS) — refusing." >&2; exit 1 ;; esac
case "$STORE_ABS/" in "$LINK_ABS"/*) echo "ERROR: store ($STORE_ABS) is inside the link dir ($LINK_ABS) — refusing." >&2; exit 1 ;; esac

REQUIRED="ramdisk.img system.img vendor.img"   # run-oh.sh hard-fails without these
OPTIONAL="updater.img sys_prod.img chip_prod.img userdata.img"

MISSING=0
for f in $REQUIRED $OPTIONAL; do
    # never follow a dangling store entry into the link dir
    if [ -L "$STORE/$f" ] && [ ! -e "$STORE/$f" ]; then
        echo "BROKEN store entry: $STORE/$f (dangling symlink)" >&2
        MISSING=1; continue
    fi
    if [ ! -f "$STORE/$f" ]; then
        echo "missing image: $STORE/$f" >&2
        MISSING=1; continue
    fi
    rm -f "$LINKDIR/$f"
    ln -s "$STORE_ABS/$f" "$LINKDIR/$f"
done
[ "$MISSING" = 0 ] || { echo "oh-image incomplete (see above)." >&2; exit 1; }

echo "OH images linked: $LINKDIR -> $STORE_ABS"
echo "Boot with: make oh-run"
