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
# Image-store rule (2026-10-30): real image bytes live in the central
# store; the worktree only keeps a symlink.
STORE="/home/william/rux-images/ubuntu"
NEXT="r54"
IMG="$STORE/gnome-gdm-${NEXT}.img"
ln -sfn "$IMG" "$WORK/gnome-gdm-${NEXT}.img"

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

echo "== systemd1shim (gdmr54: org.freedesktop.systemd1 bus owner)"
# logind's CreateSession starts the session scope through
# org.freedesktop.systemd1; without the name owned, dbus activation
# fails and no logind session can exist for the greeter (gdmr53). The
# shim is built by gdm-img.sh's full walk (cross gcc + image glib,
# headers banked in work/glibdev-riscv64); the repack refreshes it from
# the banked binary.
if [ -f "$WORK/gnome-debs-gdm/systemd1shim.bin" ]; then
    cp "$WORK/gnome-debs-gdm/systemd1shim.bin" "$ROOT/usr/libexec/systemd1shim"
    chmod 0755 "$ROOT/usr/libexec/systemd1shim"
fi

echo "== pam_systemd (gdmr52: real logind session for the greeter)"
# gdm-manager's RegisterDisplay lookup needs a logind session for the
# greeter user (sd_pid_get_session fails — no cgroups; then
# sd_uid_get_sessions(120) reads /run/systemd/users/120). The gdmr46
# sd-login bridge wrote those files from PID1, but logind OWNS
# /run/systemd/users/120 and rewrites/unlinks it on every user-120
# lifecycle event (UserNew/user_start/UserRemoved churn each greeter
# attempt — gdmr51: RegisterDisplay failed deterministically). The
# stock path is pam_systemd: the worker's pam_open_session calls
# login1 CreateSession, so LOGIND writes sessions/c1 + users/120
# itself — same files the lookup reads, owned by their real writer.
# libpam-systemd deb is banked in work/gnome-debs-gdm (jammy 249.11).
DEB="$WORK/gnome-debs-gdm/libpam-systemd_249.11-0ubuntu3.23_riscv64.deb"
if [ -f "$DEB" ]; then
    dpkg-deb -x "$DEB" "$ROOT"
    # pam-configs/systemd only takes effect via pam-auth-update (never
    # run here) — stock Ubuntu's line goes into common-session, right
    # after pam_unix:
    grep -q 'pam_systemd' "$ROOT/etc/pam.d/common-session" || \
        sed -i '/^session[[:space:]]*required[[:space:]]*pam_unix.so/a session\toptional\tpam_systemd.so' \
            "$ROOT/etc/pam.d/common-session"
else
    echo "warn: libpam-systemd deb missing — greeter session will use the stale sd-login bridge" >&2
fi

echo "== gdm-theme.gresource (update-alternatives gap)"
# gnome-shell-common's postinst installs gdm-theme.gresource via
# update-alternatives -> gnome-shell-theme.gresource (priority 10).
# dpkg-deb -x never runs postinst, so the link is absent and
# gnome-shell's reloadThemeResource (main.js:497) ENOENTs -> shell
# exits(1) -> gdm-session re-starts it forever (the gdmr48 loop).
# Stock theme link = the update-alternatives --auto choice.
ln -sfn gnome-shell-theme.gresource \
    "$ROOT/usr/share/gnome-shell/gdm-theme.gresource"

echo "== /etc/X11/xorg.conf (gdmr51: the greeter Xorg driver choice)"
# gdm's [server-X0] command passes -config /etc/X11/xorg-fbdev.conf,
# but the greeter Xorg runs as uid 120 (gdm-x-session), and Xorg only
# honors an absolute -config path for root — for every other user it
# silently falls back to autoconfig (gdmr50 Xorg.0.log: "Using system
# config directory", no "Using config file"). Autoconfig picks the
# modesetting driver on /dev/dri/card0 -> glamor -> gbm -> deterministic
# crash in the gbm DRI-driver loader on this kernel (gdmr50: SIGSEGV
# epc in libgbm.so.1.0.0 every cycle). Installing the SAME proven
# fbdev config as the SYSTEM default makes every X server on the
# image (greeter included) take fbdev + shadow framebuffer — the path
# that already lights pixels here.
if [ -f "$ROOT/etc/X11/xorg-fbdev.conf" ]; then
    cp "$ROOT/etc/X11/xorg-fbdev.conf" "$ROOT/etc/X11/xorg.conf"
fi

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
