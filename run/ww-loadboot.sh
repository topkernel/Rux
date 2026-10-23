#!/bin/bash
# ww-loadboot.sh — s31 load-diversification hunt boot (sdc-cycle surface).
#
# Identical geometry to ww-boot.sh EXCEPT the system disk: this boots the
# PRIVATE copy run/ww31-system.img, which carries one injected change —
# /system/etc/init.cfg additionally imports /system/etc/init.wwload.cfg,
# which starts the "wwload" service: /system/bin/sh /system/bin/wwload.sh,
# an endless paced loop of `timeout 2 /system/bin/sdc` exec cycles (the
# b77-era trigger context: the StorageDaemon fork/exec boundary, repeated
# under the live OH storm). Read-only discipline: global -snapshot +
# file.locking=off (guest writes, incl. the /usr mount, are discarded).
#
# Do NOT edit ww-boot.sh instead: the live chain driver executes it.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMGDIR="/home/william/rux-agents/oh-display/run/oh-img"
TAG="${TAG:-wwload}"
GDBPORT="${GDBPORT:-12381}"
SMPN="${SMP:-2}"
SECSN="${SECS:-930}"
KERNEL="${KERNEL:-$ROOT/run/ww-hunt-frozen-bzImage}"
SYSTEMIMG="${SYSTEMIMG:-$ROOT/run/ww31-system.img}"
RUN="$ROOT/run"
SERIAL="$RUN/${TAG}-serial.log"

rm -f "$SERIAL"

qemu_drive() { # idx file
    local idx=$1 file=$2
    [ -f "$file" ] || return 0
    printf ' -drive file=%s,if=none,id=ohd%d,format=raw,file.locking=off -device virtio-blk-pci,drive=ohd%d' "$file" "$idx" "$idx"
}

# userdata: tok-seeded blank image (HDF accessToken chain needs it)
USERDATA="$IMGDIR/userdata-tok.img"
[ -f "$USERDATA" ] || USERDATA="$IMGDIR/userdata-rw.img"
[ -f "$USERDATA" ] || USERDATA="$IMGDIR/userdata.img"

APPEND="ip=dhcp loglevel=7 console=ttyS0,115200"
APPEND="$APPEND init=init root=/dev/ram0 rw"
APPEND="$APPEND ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio"
APPEND="$APPEND ohos.boot.sn=01234567890"
APPEND="$APPEND ohos.required_mount.system=/dev/vdb@/usr@ext4@ro,barrier=1@wait,required"
APPEND="$APPEND ohos.required_mount.vendor=/dev/vdc@/vendor@ext4@ro,barrier=1@wait,required"
APPEND="$APPEND ${EXTRA_APPEND:-}"

DISKS="$(qemu_drive 0 "$IMGDIR/updater.img")$(qemu_drive 1 "$SYSTEMIMG")$(qemu_drive 2 "$IMGDIR/vendor.img")$(qemu_drive 3 "$IMGDIR/sys_prod.img")$(qemu_drive 4 "$IMGDIR/chip_prod.img")$(qemu_drive 5 "$USERDATA")"

RAMDISK="$IMGDIR/ramdisk-x64.img"
[ -f "$RAMDISK" ] || RAMDISK="$IMGDIR/ramdisk.img"

exec timeout --signal=TERM --kill-after=10 "${SECSN}s" qemu-system-x86_64 \
    -M q35 -accel tcg,thread=multi -cpu max -m 4G -smp "$SMPN" -snapshot \
    -gdb tcp:127.0.0.1:"$GDBPORT" \
    -display none \
    -vga none \
    -device virtio-gpu-pci,id=ohgpu,xres=720,yres=1280 \
    -serial file:"$SERIAL" \
    -kernel "$KERNEL" -initrd "$RAMDISK" \
    -append "$APPEND" \
    $DISKS \
    -nic user,model=virtio-net-pci \
    -device virtio-keyboard-pci -device virtio-tablet-pci
