#!/bin/bash
# build-img.sh — assemble the Ubuntu riscv64 GNOME image (GNOME gate).
#
# Rebuild of the lost gp3i/gnome.img chain (the recipe survives only as
# the x86 mirror in test/gnome-x86/build-img.sh). Same shape: Ubuntu
# 22.04 riscv64 base + full GNOME session chain (Xorg-fbdev + llvmpipe,
# both dbus, gnome-session + gsd + gnome-shell) under /sbin/runit.
#
# Cross-arch safe: no chroot. apt resolves the riscv64 closure from
# ubuntu-ports with APT::Architecture=riscv64, packages are unpacked
# with dpkg-deb -x, runit is cross-compiled with the repo musl
# toolchain. Output: work/gnome-riscv64.img
#
# Boot: python3 test/ubuntu-gnome/fire.py (headless ignition driver).
set -euo pipefail

WT="$(cd "$(dirname "$0")/../.." && pwd)"
DL="$WT/work/downloads"
WORK="$WT/work"
ROOT="$WORK/gnome-root-riscv64"
# Overridable so successive images can coexist (GNOME_IMG=work/gnome-r3.img ...)
IMG="${GNOME_IMG:-$WORK/gnome-riscv64.img}"
SRC_TGZ="$DL/ubuntu-base-22.04.3-base-riscv64.tar.gz"
SRC_URL="http://cdimage.ubuntu.com/ubuntu-base/releases/22.04.3/release/ubuntu-base-22.04.3-base-riscv64.tar.gz"

command -v mkfs.ext4 >/dev/null || { echo "missing mkfs.ext4" >&2; exit 1; }
command -v riscv64-linux-gnu-gcc >/dev/null || { echo "missing riscv64-linux-gnu-gcc" >&2; exit 1; }

# Repo musl toolchain (march-pinned) for runit — same reason as udesk:
# host cross-gcc static glibc assumes a newer RVA baseline than QEMU
# rv64. Falls back to plain cross-gcc if musl is absent (runit is tiny
# and does not use glibc vector paths, but keep the proven default).
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

mkdir -p "$DL" "$WORK"

echo "== unpacking base rootfs"
rm -rf "$ROOT"
mkdir -p "$ROOT"
if [ ! -s "$SRC_TGZ" ]; then
    # Reuse the main repo's cached extract if the tarball is missing
    # (read-only source; we copy).
    if [ -d /home/william/workspace/Rux/work/ubuntu-base-rootfs/22.04.3 ]; then
        echo "   (copying cached ubuntu-base extract from main work dir)"
        cp -a --reflink=auto /home/william/workspace/Rux/work/ubuntu-base-rootfs/22.04.3/. "$ROOT/"
    else
        echo "downloading $SRC_URL"
        curl -fL --retry 3 -o "$SRC_TGZ" "$SRC_URL"
        tar -xzf "$SRC_TGZ" -C "$ROOT"
    fi
else
    tar -xzf "$SRC_TGZ" -C "$ROOT"
fi

# ---------------------------------------------------------------------------
# FULL GNOME session stack: Xorg (fbdev) + software GL (llvmpipe) + dbus
# + gnome-session/gnome-shell + gnome-settings-daemon (gsd) + xterm probe.
# ---------------------------------------------------------------------------
echo "== fetching GNOME chain (apt download + dpkg-deb -x — no chroot)"
APT_SRC="$(mktemp)"
APT_LIST="$WORK/gnome-apt-lists"
APT_CACHE="$WORK/gnome-apt-cache"
mkdir -p "$APT_LIST/partial" "$APT_CACHE"
cat > "$APT_SRC" <<'EOS'
deb [trusted=yes] http://mirrors.aliyun.com/ubuntu-ports/ jammy main restricted universe multiverse
deb [trusted=yes] http://mirrors.aliyun.com/ubuntu-ports/ jammy-updates main restricted universe multiverse
EOS
# upower: gsd-power needs a live org.freedesktop.UPower on the system
# bus or it exits(1) on plugin init and gnome-session respawn-loops it
# (49 deaths per 30-min boot observed, each trip risking the "Oh no"
# error screen). upowerd is dbus-activatable, so no runit change is
# needed — the system bus starts it on first StartServiceByName.
PKGS="dash bash xserver-xorg-core xserver-xorg-video-fbdev x11-xserver-utils dbus dbus-x11 libgl1-mesa-dri libgl1-mesa-glx gnome-session-bin gnome-session gnome-shell gnome-settings-daemon librsvg2-common xterm fonts-dejavu-core util-linux coreutils procps upower"

AOPTS=(-o Dir::Etc::sourcelist="$APT_SRC" -o Dir::Etc::sourceparts=-
       -o Dir::State::lists="$APT_LIST" -o Dir::State::status=/dev/null
       -o Dir::Cache="$APT_CACHE"
       -o APT::Architecture=riscv64 -o Architectures=riscv64
       -o Acquire::AllowInsecureRepositories=true -o APT::Get::AllowUnauthenticated=true)

# Prime the jammy riscv64 indices (host may be running a different
# Ubuntu release; without private lists the resolver mixes versions).
apt-get "${AOPTS[@]}" update >/dev/null 2>&1 || true

DEBDIR="$WORK/gnome-debs-riscv64"
mkdir -p "$DEBDIR"
( cd "$DEBDIR" && apt-get "${AOPTS[@]}" download $(apt-cache "${AOPTS[@]}" \
        depends --recurse --no-recommends --no-suggests \
        --no-conflicts --no-breaks --no-replaces --no-enhances \
        $PKGS 2>/dev/null | grep "^\w" | sort -u) ) || echo "warn: some downloads failed"

echo "== extracting $(ls "$DEBDIR"/*.deb 2>/dev/null | wc -l) debs"
for d in "$DEBDIR"/*.deb; do
    dpkg-deb -x "$d" "$ROOT" 2>/dev/null || echo "warn: extract failed: $(basename "$d")"
done

# Minimal /etc the debs don't fully provide
cp /etc/resolv.conf "$ROOT/etc/resolv.conf" 2>/dev/null || true
echo rux-gnome > "$ROOT/etc/hostname"
: > "$ROOT/etc/mtab"

# dbus refuses to start without its messagebus user (dpkg -x skips
# maintainer scripts, so the adduser never ran)
grep -q '^messagebus:' "$ROOT/etc/passwd" 2>/dev/null || \
    echo 'messagebus:x:102:105::/nonexistent:/usr/sbin/nologin' >> "$ROOT/etc/passwd"
grep -q '^messagebus:' "$ROOT/etc/group" 2>/dev/null || \
    echo 'messagebus:x:105:' >> "$ROOT/etc/group"

# machine-id: gnome-shell/dbus refuse to spawn a session bus without it
python3 -c "import uuid; print(uuid.uuid4().hex)" > "$ROOT/etc/machine-id"
mkdir -p "$ROOT/var/lib/dbus"
cp "$ROOT/etc/machine-id" "$ROOT/var/lib/dbus/machine-id"

# /var/run -> /run: GLib's Gio.DBus.system defaults to the historical
# /var/run/dbus path — without the symlink every system-bus client gets
# ENOENT and gnome-shell dies during panel construction (see x86 merge
# 266ce165 for the full autopsy).
if [ ! -L "$ROOT/var/run" ]; then
    rmdir "$ROOT/var/run" 2>/dev/null || rm -rf "$ROOT/var/run"
    ln -s /run "$ROOT/var/run"
fi

# compiled GSettings schemas (dpkg -x skips glib-compile-schemas; the
# host tool is arch-independent for this step)
if command -v glib-compile-schemas >/dev/null; then
    glib-compile-schemas "$ROOT/usr/share/glib-2.0/schemas/" \
        --targetdir="$ROOT/usr/share/glib-2.0/schemas/" || true
fi

# ---------------------------------------------------------------------------
# swrast DRI driver: pin the 20.0.4 riscv64 build.
#
# The jammy (and focal-updates) riscv64 mesa builds have a broken drisw
# path: __driDriverGetExtensions_swrast + createNewScreen2 with the
# xserver's stock DRI_SWRastLoader v1 returns NULL before any syscall
# (Xorg: "(EE) IGLX error: Calling driver entry point failed / GLX:
# could not load software renderer"; verified kernel-independent —
# identical under the Oct-14 painting kernel and under qemu-user on the
# host). The 20.0.4 riscv64 driver (softpipe, no LLVM dep) passes the
# same probe (glnit: screen created). amd64 is unaffected (its builds
# expose the legacy DRI interface — the x86 mirror image works).
#
# Replacement must UNLINK first: the jammy deb's dri/ drivers arrive as
# one hardlink group, and an in-place cp over a member mutates every
# name in the group and can leave mkfs -d's hardlink bookkeeping in a
# state where the packed image references an inode the bitmap never
# marks allocated (observed on the hand-built m20 image: dirent ->
# ino 42784, bitmap bit clear, i_nlink 0 — the first mkdir of the boot
# then allocated 42784 as /tmp/xdg and swrast_dri.so resolved to a
# directory; dlopen's header pread got EISDIR and AIGLX died — see
# GNOME-SESSIONS.txt session 2). The e2fsck gate below is the safety
# net that refuses to ship such an image.
# ---------------------------------------------------------------------------
SW20_LOCAL="$DL/swrast_dri.so-20.0.4-riscv64"
SW20_DEB="$DL/libgl1-mesa-dri_20.0.4-2ubuntu1_riscv64.deb"
if [ ! -s "$SW20_LOCAL" ]; then
    echo "== obtaining mesa 20.0.4 swrast driver"
    for u in \
        "http://mirrors.aliyun.com/ubuntu-ports/pool/main/m/mesa/libgl1-mesa-dri_20.0.4-2ubuntu1_riscv64.deb" \
        "http://ports.ubuntu.com/ubuntu-ports/pool/main/m/mesa/libgl1-mesa-dri_20.0.4-2ubuntu1_riscv64.deb"; do
        curl -fL --retry 2 -o "$SW20_DEB" "$u" && break
        rm -f "$SW20_DEB"
    done
    [ -s "$SW20_DEB" ] || { echo "swrast 20.0.4 deb unobtainable" >&2; exit 1; }
    rm -rf "$WORK/sw20" && mkdir -p "$WORK/sw20"
    dpkg-deb -x "$SW20_DEB" "$WORK/sw20"
    cp "$WORK/sw20/usr/lib/riscv64-linux-gnu/dri/swrast_dri.so" "$SW20_LOCAL"
fi
rm -f "$ROOT/usr/lib/riscv64-linux-gnu/dri/swrast_dri.so" \
      "$ROOT/usr/lib/riscv64-linux-gnu/dri/kms_swrast_dri.so"
install -m 0644 "$SW20_LOCAL" "$ROOT/usr/lib/riscv64-linux-gnu/dri/swrast_dri.so"
install -m 0644 "$SW20_LOCAL" "$ROOT/usr/lib/riscv64-linux-gnu/dri/kms_swrast_dri.so"

# Client side must match the driver era: the jammy libGLX_mesa/libEGL_mesa
# (23.2) walk the driver's extension table and die with "did not find
# extension DRI_Mesa version 1 / failed to load driver: swrast" against
# the 20.0.4 swrast (the XSERVER's stock DRI loader accepts the same
# driver fine — AIGLX initializes). Pin libGLX_mesa.so.0, libEGL_mesa.so.0
# and libglapi.so.0 from the same 20.0.4 riscv64 builds (GLVND discovers
# vendor libs by soname, so only these files change). EGL matters for
# gnome-session-check-accelerated-gles-helper ("libEGL fatal: did not
# find extension DRI_Mesa version 1" kills the helper before it can
# report the renderer).
for d in libglx-mesa0_20.0.4-2ubuntu1_riscv64.deb libglapi-mesa_20.0.4-2ubuntu1_riscv64.deb libegl-mesa0_20.0.4-2ubuntu1_riscv64.deb; do
    if [ ! -s "$DL/$d" ]; then
        curl -fL --retry 2 -o "$DL/$d" \
            "http://mirrors.aliyun.com/ubuntu-ports/pool/main/m/mesa/$d"
    fi
done
rm -rf "$WORK/sw20client" && mkdir -p "$WORK/sw20client"
dpkg-deb -x "$DL/libglx-mesa0_20.0.4-2ubuntu1_riscv64.deb" "$WORK/sw20client"
dpkg-deb -x "$DL/libglapi-mesa_20.0.4-2ubuntu1_riscv64.deb" "$WORK/sw20client"
dpkg-deb -x "$DL/libegl-mesa0_20.0.4-2ubuntu1_riscv64.deb" "$WORK/sw20client"
for lib in libGLX_mesa.so.0.0.0 libglapi.so.0.0.0 libEGL_mesa.so.0.0.0; do
    rm -f "$ROOT/usr/lib/riscv64-linux-gnu/$lib"
    install -m 0644 "$WORK/sw20client/usr/lib/riscv64-linux-gnu/$lib" \
        "$ROOT/usr/lib/riscv64-linux-gnu/$lib"
done

# Accept software GL for the stock session: gnome-session-check-accelerated
# matches the renderer against hardware-compatibility and the stock file
# ends with "-softpipe" (upstream enables llvmpipe only). The pinned
# 20.0.4 riscv64 driver IS softpipe (built without LLVM), so the last
# rule must whitelist it or the session refuses to start ("No hardware
# 3D support" -> gnome-session-failed dialog -> fallback shell without
# gsd). Rules apply in order with last-match-wins: append +softpipe.
cat >> "$ROOT/usr/share/gnome-session/hardware-compatibility" <<'EOH'

# Rux bring-up: softpipe is the only swrast on this platform
+softpipe
EOH

# ---------------------------------------------------------------------------
# /sbin/runit — the session bootstrap (clear stale X locks, both dbus
# daemons, Xorg on fbdev, then the full gnome-session chain).
# ---------------------------------------------------------------------------
CC "$ROOT/sbin/runit" "$WT/test/ubuntu-gnome/runit.c"
# Default knobs: full stock chain. /etc/runit-mode "gs" boots the
# unsupervised gnome-shell --replace path instead (TCG-friendly).
echo session > "$ROOT/etc/runit-mode"
echo gnome > "$ROOT/etc/runit-session"

# gsd-usb-protection crash-loop mitigation. The daemon dereferences a
# NULL GError on its "Failed to connect to screensaver service" warning
# path (gsd 42.9 gsd-usb-protection.c assumes error != NULL from
# g_dbus_proxy_new_for_bus_finish) and takes SIGSEGV; X-GNOME-AutoRestart
# then respawns it every few seconds for the whole session (27+ crashes
# per boot observed). USB protection needs UPower/usbguard which this
# image does not ship anyway — drop it from the session's required
# components and hide its autostart entry.
SESS="$ROOT/usr/share/gnome-session/sessions/gnome.session"
sed -i 's/org\.gnome\.SettingsDaemon\.UsbProtection;//' "$SESS"
USBDE="$ROOT/etc/xdg/autostart/org.gnome.SettingsDaemon.UsbProtection.desktop"
if [ -f "$USBDE" ]; then
    printf 'Hidden=true\n' >> "$USBDE"
fi

# Xorg fbdev config (software rendering, no KMS/DRM — Rux has fbdev only)
mkdir -p "$ROOT/etc/X11"
cat > "$ROOT/etc/X11/xorg-fbdev.conf" <<'EOX'
Section "Device"
    Identifier "RuxFbdev"
    Driver     "fbdev"
EndSection
Section "Monitor"
    Identifier "Monitor0"
EndSection
Section "Screen"
    Identifier "Screen0"
    Device "RuxFbdev"
    Monitor "Monitor0"
    DefaultDepth 24
    SubSection "Display"
        Depth 24
        Modes "1280x800"
    EndSubSection
EndSection
EOX

# ---------------------------------------------------------------------------
# Pack the image (Rux ext4 constraints: 4K blocks, no metadata_csum).
# The full GNOME chain unpacks to ~2.5G — give the fs 3.5G.
#
# fakeroot pass: dpkg-deb -x runs as a plain user, so the staging tree
# is 1000:1000 throughout and mkfs -d would bake that into the image.
# Two things break downstream: (a) system-bus activation refuses to
# exec the setuid launch helper unless it is root-owned with the suid
# bit — every StartServiceByName(org.freedesktop.*) died with "The
# permission of the setuid helper is not correct" (83-191 lines per
# boot; gsd-power/keyboard/media-keys all affected); (b) nothing on
# the image is root-owned at all, which no stock rootfs does. Fix by
# re-owning the tree 0:0 under fakeroot and setting the helper to its
# stock Ubuntu mode 4754 root:messagebus, then packing inside the same
# fakeroot session so mkfs -d records the faked ownership.
# ---------------------------------------------------------------------------
HELPER=""
for c in "$ROOT/usr/lib/dbus-1.0/dbus-daemon-launch-helper" \
         "$ROOT/usr/lib/dbus-1/dbus-daemon-launch-helper"; do
    [ -f "$c" ] && HELPER="${c#"$ROOT"}"
done
[ -n "$HELPER" ] || { echo "dbus launch helper not found in staging tree" >&2; exit 1; }
command -v fakeroot >/dev/null || { echo "missing fakeroot" >&2; exit 1; }

echo "== packing $IMG (fakeroot: root-owned tree, $HELPER 4754 root:messagebus)"
export ROOT IMG HELPER
fakeroot -- sh -c '
    chown -R 0:0 "$ROOT"
    chown 0:105 "$ROOT$HELPER"
    chmod 4754 "$ROOT$HELPER"
    mkfs.ext4 -b 4096 -O ^metadata_csum -d "$ROOT" "$IMG" 3500M >/dev/null
'

# e2fsck gate: mkfs -d's hardlink bookkeeping can, on a surgically
# modified root, pack a dirent whose inode the bitmap never marks
# allocated (see the swrast comment above — the running system's first
# create then aliases onto the live file). e2fsck reallocates the
# referenced inode and fixes nlink; exit codes 0/1 are success, anything
# else aborts the build rather than shipping a self-corrupting image.
echo "== e2fsck gate"
FSCK_RC=0
e2fsck -fy "$IMG" >/dev/null 2>&1 || FSCK_RC=$?
if [ "$FSCK_RC" -gt 1 ]; then
    echo "e2fsck rc=$FSCK_RC — image NOT shippable" >&2
    exit 1
fi

echo "image ready: $IMG ($(du -h "$IMG" | cut -f1))"
