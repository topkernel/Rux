#!/bin/bash
# build-img.sh — pack the real Ubuntu 24.04 LTS desktop rootfs into a
# bootable ext4 image for Rux x86_64 (systemd as PID 1, gdm3 target).
#
# Source of truth: the full noble desktop rootfs extracted at
# $ROOTFS_SRC (default /home/william/rux-agents/ubuntu2404/rootfs2 —
# debootstrap noble + ubuntu-desktop, ~1500 packages, gdm3/gnome-shell/
# systemd 255). That tree lives OUTSIDE any git worktree, like all image
# assets; this script only reads it.
#
# First-boot prep applied here (each item is standard admin prep, not a
# behavioural change):
#   - /etc/fstab: /dev/vda root line (debootstrap ships "UNCONFIGURED")
#   - /etc/resolv.conf: QEMU user-net resolver 10.0.2.3
#   - /etc/hostname: ub2404
#   - serial-getty@ttyS0 enabled (serial console = the bring-up channel)
#   - root password set to "rux" (serial login; source tree carries an
#     unknown install-time hash)
#   - cloud-init disabled (/etc/cloud/cloud-init.disabled) — its
#     datasource hunt adds minutes of noise to first boot
#   - apt package cache dropped (~830M), /var/cache/apt/lists kept
#   - fresh /var/lib/dbus/machine-id mirroring /etc/machine-id
#
# Output: $IMG (default /home/william/rux-images/ubuntu/ub2404.img) —
# real image bytes NEVER live inside a worktree (image-store rule).
# Boot: run/boot-ub2404.sh (q35, kvm, smp8, serial to file).
#
# Rux ext4 constraints: 4K blocks, no metadata_csum. Swap tail carve
# mirrors scripts/test/ubuntu-x86/build-img.sh (Kernel.toml
# swap_size_mb=256): filesystem never resized into the tail.
#
# Needs sudo ONLY for the staging copy (root ownership) and mkfs -d
# (uid/gid fidelity); the output image is chowned back to the caller.
set -euo pipefail

WT="$(cd "$(dirname "$0")" && pwd)"
while [ ! -f "$WT/Cargo.toml" ] || [ ! -d "$WT/kernel" ]; do
    WT="$(dirname "$WT")"
done
cd "$WT"

ROOTFS_SRC="${ROOTFS_SRC:-/home/william/rux-agents/ubuntu2404/rootfs2}"
IMG="${IMG:-/home/william/rux-images/ubuntu/ub2404.img}"
FS_SIZE="${FS_SIZE:-6G}"
SWAP_TAIL_MB="${SWAP_TAIL_MB:-300}"
ROOT_PASS="${ROOT_PASS:-rux}"

command -v mkfs.ext4 >/dev/null || { echo "missing mkfs.ext4" >&2; exit 1; }
[ -x "$ROOTFS_SRC/usr/lib/systemd/systemd" ] || [ -d "$ROOTFS_SRC/usr/lib/systemd/systemd" ] || {
    echo "rootfs source looks wrong: $ROOTFS_SRC (no systemd)" >&2; exit 1; }
[ -f "$ROOTFS_SRC/usr/sbin/gdm3" ] || { echo "no gdm3 in rootfs" >&2; exit 1; }

STAGE="$WT/work/ub2404-stage"
echo "== staging rootfs (rsync -aHAX, excludes: apt cache debs)"
sudo rm -rf "$STAGE"
sudo mkdir -p "$STAGE"
sudo rsync -aHAX --delete \
    --exclude='/var/cache/apt/archives/*.deb' \
    --exclude='/var/cache/apt/archives/partial/' \
    --exclude='/proc/*' --exclude='/sys/*' --exclude='/dev/*' --exclude='/run/*' \
    "$ROOTFS_SRC"/ "$STAGE"/

echo "== first-boot prep"
# fstab: NO root line — PID 1 mounts / from the kernel cmdline
# (root=/dev/vda) and a fstab root entry makes systemd's -.mount unit
# re-verify the mount against /proc/self/mountinfo and fail (first-boot
# lesson: "Failed to mount -.mount" cascaded into journald via
# local-fs dependencies). /run, /proc, /sys, /dev are mounted by PID 1.
sudo tee "$STAGE/etc/fstab" >/dev/null <<'EOS'
tmpfs     /tmp    tmpfs  defaults,nosuid,nodev 0 0
EOS

# QEMU user-net resolver (rootfs ships a symlink into /run/systemd/resolve
# — replace with a plain file; systemd-resolved is not the bring-up path)
sudo rm -f "$STAGE/etc/resolv.conf"
printf 'nameserver 10.0.2.3\n' | sudo tee "$STAGE/etc/resolv.conf" >/dev/null
echo ub2404 | sudo tee "$STAGE/etc/hostname" >/dev/null

# serial getty on ttyS0 — the serial console is the progress channel
sudo mkdir -p "$STAGE/etc/systemd/system/getty.target.wants"
sudo ln -sf /lib/systemd/system/serial-getty@.service \
    "$STAGE/etc/systemd/system/getty.target.wants/serial-getty@ttyS0.service"
# ... and drop its dev-ttyS0.device requirement: /dev/ttyS0 is a static
# devfs node on Rux (kernel-created, no udev tagging), so the udev device
# unit never activates and the getty would wait on it forever.
sudo mkdir -p "$STAGE/etc/systemd/system/serial-getty@ttyS0.service.d"
sudo tee "$STAGE/etc/systemd/system/serial-getty@ttyS0.service.d/override.conf" >/dev/null <<'EOS'
[Unit]
Requires=
EOS
# agetty needs the tty to exist; also mask the "container" heuristic that
# would skip auto-console gettys
[ -e "$STAGE/dev/ttyS0" ] || sudo touch "$STAGE/dev/ttyS0" 2>/dev/null || true

# root password for serial login (source tree hash is unknown)
HASH="$(openssl passwd -6 "$ROOT_PASS")"
sudo sed -i "s|^root:[^:]*:|root:${HASH//:/:}:|" "$STAGE/etc/shadow" ||
    { echo "failed to set root password" >&2; exit 1; }

# cloud-init off (first-boot datasource hunt = minutes of noise)
sudo mkdir -p "$STAGE/etc/cloud"
sudo touch "$STAGE/etc/cloud/cloud-init.disabled"

# machine-id: keep the extracted one; mirror it for dbus
MID="$(sudo cat "$STAGE/etc/machine-id")"
if [ -z "$MID" ] || [ "$MID" = "uncommitted" ]; then
    MID="$(python3 -c 'import uuid;print(uuid.uuid4().hex)')"
    echo "$MID" | sudo tee "$STAGE/etc/machine-id" >/dev/null
fi
sudo mkdir -p "$STAGE/var/lib/dbus"
echo "$MID" | sudo tee "$STAGE/var/lib/dbus/machine-id" >/dev/null

# /var/run -> /run symlink (systemd expects the usr-merged layout)
if [ ! -L "$STAGE/var/run" ]; then
    sudo rm -rf "$STAGE/var/run"
    sudo ln -s /run "$STAGE/var/run"
fi

# /etc/mtab -> /proc/self/mounts (the layout real Ubuntu ships; the
# debootstrap tree carries no mtab at all, and libmount's unit-context
# mount tracking behaves differently without it)
sudo rm -f "$STAGE/etc/mtab"
sudo ln -s /proc/self/mounts "$STAGE/etc/mtab"

echo "== packing $IMG (mkfs.ext4 -d, ${FS_SIZE} fs + ${SWAP_TAIL_MB}M swap tail)"
mkdir -p "$(dirname "$IMG")"
sudo rm -f "$IMG"
sudo mkfs.ext4 -b 4096 -O ^metadata_csum -I 256 -d "$STAGE" \
    "$IMG" "$FS_SIZE" >/dev/null
sudo truncate -s "+${SWAP_TAIL_MB}M" "$IMG"
sudo chown "${CALLER_UID:-$(id -u)}:${CALLER_GID:-$(id -g)}" "$IMG" 2>/dev/null || true

echo "image ready: $IMG ($(du -h "$IMG" | cut -f1) allocated, sparse)"
file "$IMG"
