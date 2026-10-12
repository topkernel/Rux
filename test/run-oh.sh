#!/bin/bash
# run-oh.sh — boot an OpenHarmony x86_64_virt emulator image on the Rux kernel.
#
# Bring the six virtio disks, the initrd ramdisk, and the ohos.* cmdline
# tokens up under our kernel on QEMU q35, mirroring the reference qemu_run
# scripts in the OH tree (qemu_run_user.sh / qemu_run_bridge.sh) minus KVM
# (bring-up runs under TCG first; KVM is a later perf step).
#
# Usage:
#   ./test/run-oh.sh <oh-images-dir> [mode]
#     <oh-images-dir>  directory containing the built OH images:
#                      ramdisk.img, updater.img, system.img, vendor.img,
#                      sys_prod.img, chip_prod.img, userdata.img
#     mode             console (default) | gui | debug
#
# Environment:
#   KERNEL  — path to the Rux bzImage (default: target/.../bzImage)
#   EXTRA_APPEND — appended verbatim to the kernel cmdline
set -euo pipefail

IMGDIR="${1:?usage: run-oh.sh <oh-images-dir> [console|gui|debug]}"
MODE="${2:-console}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
KERNEL="${KERNEL:-$ROOT/target/x86_64-unknown-none/debug/bzImage}"

for f in ramdisk.img system.img vendor.img; do
    [ -f "$IMGDIR/$f" ] || { echo "missing $IMGDIR/$f" >&2; exit 1; }
done
# Optional disks: warn, keep going (updater/sys_prod/chip_prod/userdata).
for f in updater.img sys_prod.img chip_prod.img userdata.img; do
    [ -f "$IMGDIR/$f" ] || echo "note: $f absent (optional)" >&2
done

# Disk order matches the reference boot log's vdb..vdg mapping:
#   vda updater  vdb system  vdc vendor  vdd sys_prod  vde chip_prod  vdf userdata
qemu_drive() { # idx file
    local idx=$1 file=$2
    [ -f "$file" ] || return 0
    printf ' -drive file=%s,if=none,id=ohd%d,format=raw,file.locking=off -device virtio-blk-pci,drive=ohd%d' "$file" "$idx" "$idx"
}

DISKS="$(qemu_drive 0 "$IMGDIR/updater.img")$(qemu_drive 1 "$IMGDIR/system.img")$(qemu_drive 2 "$IMGDIR/vendor.img")$(qemu_drive 3 "$IMGDIR/sys_prod.img")$(qemu_drive 4 "$IMGDIR/chip_prod.img")$(qemu_drive 5 "$IMGDIR/userdata.img")"

# The reference cmdline, verbatim tokens the userspace init parses
# (kernel only relays /proc/cmdline; it must not filter ohos.*).
APPEND="root=/dev/ram0 init=init console=ttyS0 rw"
APPEND="$APPEND ohos.boot.required_mount.system=/dev/block/vdb@/usr@ext4@ro,barrier=1"
APPEND="$APPEND ohos.boot.required_mount.vendor=/dev/block/vdc@/vendor@ext4@ro,barrier=1"
APPEND="$APPEND hardware=virt default_boot_device=a"
APPEND="$APPEND ${EXTRA_APPEND:-}"

QEMU="${QEMU:-qemu-system-x86_64}"
case "$MODE" in
    console) DISP="-display none" ;;
    gui)     DISP="-display gtk -device virtio-gpu-pci,xres=720,yres=1280" ;;
    debug)   DISP="-display none -s -S" ;;
    *) echo "unknown mode: $MODE" >&2; exit 1 ;;
esac

# shellcheck disable=SC2086
exec "$QEMU" \
    -M q35 -accel tcg,thread=multi -cpu max -m 4G -smp 2 \
    $DISP -serial mon:stdio -monitor none \
    -kernel "$KERNEL" -initrd "$IMGDIR/ramdisk.img" \
    -append "$APPEND" \
    $DISKS \
    -nic user,model=virtio-net-pci \
    -device virtio-keyboard-pci -device virtio-tablet-pci
