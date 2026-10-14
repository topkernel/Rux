#!/bin/bash
# build-img.sh — assemble the Ubuntu amd64 GNOME image (x86_64 GNOME gate).
#
# The x86 mirror of the riscv64 GNOME chain (runs/gp3i.img family:
# Xorg-fbdev + both dbus + gnome-shell under /sbin/runit). Host is amd64,
# so the rootfs is assembled with a NATIVE chroot — no qemu-user needed.
#
# Output: work/gnome-x86.img (ext4, 4K blocks, no metadata_csum — Rux
# mount requirements). Boot with: KERNEL=... ./test/run-x86-ub.sh gui
#   ROOTFS=work/gnome-x86.img
set -euo pipefail

WT="$(cd "$(dirname "$0")/../.." && pwd)"
DL="$WT/work/downloads"
WORK="$WT/work"
ROOT="$WORK/gnome-root-x86"
IMG="$WORK/gnome-x86.img"
SRC_TGZ="$DL/ubuntu-base-22.04-base-amd64.tar.gz"
SRC_URL="http://cdimage.ubuntu.com/ubuntu-base/releases/22.04/release/ubuntu-base-22.04-base-amd64.tar.gz"

command -v mkfs.ext4 >/dev/null || { echo "missing mkfs.ext4" >&2; exit 1; }
[ "$(uname -m)" = "x86_64" ] || { echo "host must be amd64 for the native chroot" >&2; exit 1; }

mkdir -p "$DL"
if [ ! -s "$SRC_TGZ" ]; then
    echo "downloading $SRC_URL"
    curl -fL --retry 3 -o "$SRC_TGZ" "$SRC_URL"
fi

echo "== unpacking base rootfs"
rm -rf "$ROOT"
mkdir -p "$ROOT"
tar -xzf "$SRC_TGZ" -C "$ROOT"

# ---------------------------------------------------------------------------
# Minimal GNOME session stack (mirrors the riscv64 gp3i chain):
#   Xorg (fbdev) + software GL (llvmpipe) + dbus + gnome-session/gnome-shell
# ---------------------------------------------------------------------------
echo "== fetching GNOME chain (apt download + dpkg-deb -x — no chroot needed)"
APT_SRC="$(mktemp)"
APT_LIST="$(mktemp -d)"
cat > "$APT_SRC" <<'EOS'
deb [trusted=yes] http://mirrors.aliyun.com/ubuntu/ jammy main restricted universe multiverse
deb [trusted=yes] http://mirrors.aliyun.com/ubuntu/ jammy-updates main restricted universe multiverse
EOS
PKGS="xserver-xorg-core xserver-xorg-video-fbdev x11-xserver-utils dbus dbus-x11 libgl1-mesa-dri libgl1-mesa-glx gnome-session-bin gnome-shell fonts-dejavu-core util-linux coreutils"

# Resolve the full recursive dependency closure (no recommends), download
# every .deb, and extract into the rootfs. dpkg-deb -x needs no privileges;
# the missing maintainer-script work (alternatives, ldconfig cache,
# fontconfig caches) is either non-essential or regenerable at runtime.
DEBDIR="$WORK/gnome-debs"
mkdir -p "$DEBDIR" "$APT_LIST/partial"
APT_CACHE="$WORK/gnome-apt-cache"
mkdir -p "$APT_CACHE"
# Prime the jammy indices into our private lists dir first (host is noble;
# without this the resolver mixes noble versions into a jammy download set).
apt-get \
    -o Dir::Etc::sourcelist="$APT_SRC" -o Dir::Etc::sourceparts=- \
    -o Dir::State::lists="$APT_LIST" -o Dir::State::status=/dev/null \
    -o Dir::Cache="$APT_CACHE" \
    -o Acquire::AllowInsecureRepositories=true update >/dev/null 2>&1 || true
( cd "$DEBDIR" && apt-get \
    -o Dir::Etc::sourcelist="$APT_SRC" -o Dir::Etc::sourceparts=- \
    -o Dir::State::lists="$APT_LIST" -o Dir::State::status=/dev/null \
    -o Dir::Cache="$APT_CACHE" \
    -o APT::Get::AllowUnauthenticated=true \
    download $(apt-cache \
        -o Dir::Etc::sourcelist="$APT_SRC" -o Dir::Etc::sourceparts=- \
        -o Dir::State::lists="$APT_LIST" -o Dir::State::status=/dev/null \
        -o Dir::Cache="$APT_CACHE" \
        depends --recurse --no-recommends --no-suggests \
        --no-conflicts --no-breaks --no-replaces --no-enhances \
        $PKGS 2>/dev/null | grep "^\w" | sort -u) )

echo "== extracting $(ls "$DEBDIR"/*.deb | wc -l) debs"
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

# ---------------------------------------------------------------------------
# /sbin/runit — the session bootstrap (same shape as the riscv64 gp3i image:
# clear stale X locks, both dbus daemons, Xorg on fbdev, then shell out so
# the serial console stays usable for bring-up).
# ---------------------------------------------------------------------------
gcc -static -O2 -Wall -o "$ROOT/sbin/runit" "$WT/test/gnome-x86/runit.c"

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
    Device     "RuxFbdev"
    Monitor    "Monitor0"
    DefaultDepth 24
    SubSection "Display"
        Depth 24
        Modes "1280x800"
    EndSubSection
EndSection
EOX

# ---------------------------------------------------------------------------
# Pack the image (Rux ext4 constraints: 4K blocks, no metadata_csum).
# Size: the GNOME chain unpacks to ~2.2G — give the fs 3.5G.
# ---------------------------------------------------------------------------
echo "== packing $IMG (mkfs -d, no loop mounts needed)"
# mkfs.ext4 -d populates directly from the directory tree — no root, no
# loop mounts (the build host has no sudo).
mkfs.ext4 -b 4096 -O ^metadata_csum -d "$ROOT" "$IMG" 3G >/dev/null

echo "image ready: $IMG ($(du -h "$IMG" | cut -f1))"
