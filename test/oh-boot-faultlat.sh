#!/bin/bash
# oh-boot-faultlat.sh — headless OH boot for the faultlat lane.
#
# Same QEMU command as oh-display's run/oh-boot-seeded.sh (q35, 720x1280
# virtio-gpu, six disks, ohos.* tokens, patched ramdisk-x64, seeded
# userdata), but self-contained under THIS worktree: serial log and the
# monitor socket land in run/, images are symlinked from the oh-robot
# build (userdata is a reflink copy of the seeded image so -snapshot
# never touches the shared original).
#
# Usage: oh-boot-faultlat.sh <tag> [kernel]
# Env: ACCEL (default kvm), SMP (2), MEM (4G), EXTRA_APPEND (dfx= flags)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMGDIR="$ROOT/run/oh-img"
TAG="${1:?usage: oh-boot-faultlat.sh <tag> [kernel]}"
KERNEL="${2:-$ROOT/target/x86_64-unknown-none/debug/bzImage}"
RUN="$ROOT/run"
SERIAL="$RUN/oh-$TAG-serial.log"
MON="$RUN/oh-$TAG-mon.sock"

rm -f "$SERIAL" "$MON"

qemu_drive() { # idx file
    local idx=$1 file=$2
    [ -f "$file" ] || return 0
    printf ' -drive file=%s,if=none,id=ohd%d,format=raw,file.locking=off -device virtio-blk-pci,drive=ohd%d' "$file" "$idx" "$idx"
}

USERDATA="$IMGDIR/userdata-seeded.img"
[ -f "$USERDATA" ] || { echo "missing $USERDATA (cp --reflink=auto from oh-display)" >&2; exit 1; }

APPEND="ip=dhcp loglevel=${LOGLEVEL:-7} console=ttyS0,115200"
APPEND="$APPEND init=init root=/dev/ram0 rw"
APPEND="$APPEND ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio"
APPEND="$APPEND ohos.boot.sn=01234567890"
APPEND="$APPEND ohos.required_mount.system=/dev/block/vdb@/usr@ext4@ro,barrier=1@wait,required"
APPEND="$APPEND ohos.required_mount.vendor=/dev/block/vdc@/vendor@ext4@ro,barrier=1@wait,required"
APPEND="$APPEND ${EXTRA_APPEND:-}"

DISKS="$(qemu_drive 0 "$IMGDIR/updater.img")$(qemu_drive 1 "$IMGDIR/system.img")$(qemu_drive 2 "$IMGDIR/vendor.img")$(qemu_drive 3 "$IMGDIR/sys_prod.img")$(qemu_drive 4 "$IMGDIR/chip_prod.img")$(qemu_drive 5 "$USERDATA")"

# Patched ramdisk (ld-musl-x86_64.path for setuid service lib resolution)
RAMDISK="$IMGDIR/ramdisk-x64.img"
[ -f "$RAMDISK" ] || RAMDISK="$IMGDIR/ramdisk.img"

SMPN="${SMP:-2}"
ACCEL_ARGS="${ACCEL:-kvm}"

# shellcheck disable=SC2086
exec qemu-system-x86_64 \
    -M q35 -accel "$ACCEL_ARGS" -cpu max -m "${MEM:-4G}" -smp "$SMPN" -snapshot \
    -display none \
    -vga none \
    -device virtio-gpu-pci,id=ohgpu,xres=720,yres=1280 \
    -serial file:"$SERIAL" \
    -monitor unix:"$MON",server=on,wait=off \
    -kernel "$KERNEL" -initrd "$RAMDISK" \
    -append "$APPEND" \
    $DISKS \
    -nic user,model=virtio-net-pci \
    -device virtio-keyboard-pci -device virtio-tablet-pci
