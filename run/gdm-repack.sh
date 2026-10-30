#!/bin/bash
# gdm-repack.sh — repack the gdm image from the EXISTING staging tree.
#
# The full gdm-img.sh walk (staging copy from gnome-root-riscv64 + apt
# download + deb extract + fixups) takes ~35 min and re-derives a tree
# that is already sitting complete in work/gnome-root-gdm. This script
# only refreshes the two things that drift after a full build:
#   1. /sbin/runit — recompiled from test/ubuntu-gnome/runit.c (the
#      gdmr46/48-era sd-login state bridge lives there)
#   2. glib schemas — glib-compile-schemas over the tree
# then does the same fakeroot pack + e2fsck gate as gdm-img.sh.
# Output: work/gnome-gdm-r49.img (per-boot tag naming, r49 = next boot).
set -euo pipefail
WT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$WT/work"
ROOT="$WORK/gnome-root-gdm"
IMG="$WORK/gnome-gdm-r49.img"

[ -d "$ROOT" ] || { echo "missing staging tree $ROOT" >&2; exit 1; }

MUSL="$WT/toolchain/riscv64-rux-linux-musl"
if [ -f "$MUSL/lib/crt1.o" ]; then
    CC() {
        riscv64-linux-gnu-gcc -static -march=rv64gc_zicsr -O2 -nostdinc \
            -isystem /usr/lib/gcc-cross/riscv64-linux-gnu/13/include \
            -isystem "$MUSL/include" -nostdlib "$MUSL/lib/crt1.o" \
            -L"$MUSL/lib" -o "$1" "$2" -lc -lgcc
    }
else
    CC() { riscv64-linux-gnu-gcc -static -O2 -o "$1" "$2"; }
fi

echo "== runit (recompile from current runit.c)"
CC "$ROOT/sbin/runit" "$WT/test/ubuntu-gnome/runit.c"

echo "== gdm-theme.gresource (update-alternatives gap)"
# gnome-shell-common's postinst installs gdm-theme.gresource via
# update-alternatives -> gnome-shell-theme.gresource (priority 10).
# dpkg-deb -x never runs postinst, so the link is absent and
# gnome-shell's reloadThemeResource (main.js:497) ENOENTs -> shell
# exits(1) -> gdm-session re-starts it forever (the gdmr48 loop).
# Stock theme link = the update-alternatives --auto choice.
ln -sfn gnome-shell-theme.gresource \
    "$ROOT/usr/share/gnome-shell/gdm-theme.gresource"

echo "== gsettings schemas refresh"
command -v glib-compile-schemas >/dev/null && \
    glib-compile-schemas "$ROOT/usr/share/glib-2.0/schemas/" \
        --targetdir="$ROOT/usr/share/glib-2.0/schemas/" || true

echo "== pack (fakeroot: root-owned tree, suid bits)"
HELPER=""
for c in "$ROOT/usr/lib/dbus-1.0/dbus-daemon-launch-helper" \
         "$ROOT/usr/lib/dbus-1/dbus-daemon-launch-helper"; do
    [ -f "$c" ] && HELPER="${c#"$ROOT"}"
done
[ -n "$HELPER" ] || { echo "dbus launch helper not found" >&2; exit 1; }
WORKER="/usr/libexec/gdm-session-worker"
[ -f "$ROOT$WORKER" ] || { echo "gdm-session-worker not found" >&2; exit 1; }
export ROOT IMG HELPER WORKER
fakeroot -- sh -c '
    chown -R 0:0 "$ROOT"
    chown 0:105 "$ROOT$HELPER";  chmod 4754 "$ROOT$HELPER"
    chown 0:0 "$ROOT$WORKER";    chmod 4755 "$ROOT$WORKER"
    chown -R 120:120 "$ROOT/var/lib/gdm"
    chown -R 1000:1000 "$ROOT/home/rux"
    mkfs.ext4 -b 4096 -O ^metadata_csum -d "$ROOT" "$IMG" 3800M >/dev/null
'

echo "== e2fsck gate"
FSCK_RC=0
e2fsck -fy "$IMG" >/dev/null 2>&1 || FSCK_RC=$?
[ "$FSCK_RC" -gt 1 ] && { echo "e2fsck rc=$FSCK_RC — NOT shippable" >&2; exit 1; }

echo "image ready: $IMG ($(du -h "$IMG" | cut -f1))"
