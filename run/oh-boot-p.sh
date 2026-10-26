#!/bin/bash
# oh-boot-w.sh — Rux OH boot with a WRITABLE private userdata copy.
# Modeled on oh-boot-m3g.sh but without the global -snapshot: shared
# system/vendor/etc images get per-drive snapshot=on + file.locking=off,
# the userdata image is written for real (private copy, never shared).
#
# Usage: oh-boot-w.sh <tag> <userdata-img> [kernel]
# Env: SMP=2 SECS=<boot seconds> EXTRA_APPEND=...
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMGDIR="$ROOT/run/oh-img-p"
TAG="${1:?usage: oh-boot-w.sh <tag> <userdata> [kernel]}"
USERDATA="${2:?userdata}"
KERNEL="${3:-$ROOT/target/x86_64-unknown-none/debug/bzImage}"
RUN="$ROOT/run"
SERIAL="$RUN/oh-$TAG-serial.log"
MON="$RUN/oh-$TAG-mon.sock"
PIDFILE="$RUN/oh-$TAG-qemu.pid"

rm -f "$SERIAL" "$MON" "$PIDFILE"

qemu_drive() { # idx file (shared, read-only overlay)
    local idx=$1 file=$2
    [ -f "$file" ] || return 0
    printf ' -drive file=%s,if=none,id=ohd%d,format=raw,file.locking=off,snapshot=on -device virtio-blk-pci,drive=ohd%d' "$file" "$idx" "$idx"
}

APPEND="ip=dhcp loglevel=7 console=ttyS0,115200"
APPEND="$APPEND init=init root=/dev/ram0 rw"
APPEND="$APPEND ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio"
APPEND="$APPEND ohos.boot.sn=01234567890"
APPEND="$APPEND ohos.required_mount.system=/dev/block/vdb@/usr@ext4@ro,barrier=1@wait,required"
APPEND="$APPEND ohos.required_mount.vendor=/dev/block/vdc@/vendor@ext4@ro,barrier=1@wait,required"
APPEND="$APPEND ${EXTRA_APPEND:-}"

DISKS="$(qemu_drive 0 "$IMGDIR/updater.img")$(qemu_drive 1 "$IMGDIR/system.img")$(qemu_drive 2 "$IMGDIR/vendor.img")$(qemu_drive 3 "$IMGDIR/sys_prod.img")$(qemu_drive 4 "$IMGDIR/chip_prod.img")"

RAMDISK="$IMGDIR/ramdisk-x64.img"
[ -f "$RAMDISK" ] || RAMDISK="$IMGDIR/ramdisk.img"

SMPN="${SMP:-2}"
SECS="${SECS:-1800}"

# shellcheck disable=SC2086
timeout --signal=TERM --kill-after=10 "${SECS}s" qemu-system-x86_64 \
    -M q35 -accel kvm -cpu max -m 4096M -smp "$SMPN" \
    -display none \
    -vga none \
    -device virtio-gpu-pci,id=ohgpu,xres=720,yres=1280 \
    -serial file:"$SERIAL" \
    -monitor unix:"$MON",server=on,wait=off \
    -pidfile "$PIDFILE" \
    -kernel "$KERNEL" -initrd "$RAMDISK" \
    -append "$APPEND" \
    $DISKS \
    -drive "if=none,id=ohd5,format=raw,file.locking=off,file=$USERDATA" -device virtio-blk-pci,drive=ohd5 \
    -nic user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:${HDCPORT:-12057}-:10178 \
    -device virtio-keyboard-pci -device virtio-tablet-pci

echo "oh-boot-w: finished after ${SECS}s (log: $SERIAL)"
