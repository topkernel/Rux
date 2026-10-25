#!/bin/bash
# hc-boot.sh — hicollie-condvar watchpoint hunt boot (leg-21).
# Same geometry as oh-boot-usbless.sh but TCG (watchpoints are reliable
# under TCG's gdbstub) + -gdb tcp (guest free-runs until gdb attaches;
# the hunt attaches only after the serial shows libhicollie mapped).
#
# Env: TAG GDBPORT SMP SECS KERNEL (defaults below); userdata is a
# private reflink copy, never the shared gold.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMGDIR="$ROOT/run/oh-img-p"
TAG="${TAG:-hc1}"
GDBPORT="${GDBPORT:-12601}"
SMPN="${SMP:-2}"
SECSN="${SECS:-3600}"
KERNEL="${KERNEL:-$ROOT/target/x86_64-unknown-none/debug/bzImage}"
RUN="$ROOT/run"
SERIAL="$RUN/${TAG}-serial.log"
USERDATA="$RUN/${TAG}-userdata.img"

rm -f "$SERIAL"
[ -f "$USERDATA" ] || cp --reflink=auto "$ROOT/run/userdata-scgold.img" "$USERDATA"

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

# shellcheck disable=SC2086
timeout --signal=TERM --kill-after=10 "${SECSN}s" qemu-system-x86_64 \
    -M q35 -accel tcg,thread=multi -cpu max -m 4096M -smp "$SMPN" \
    -display none \
    -vga none \
    -device virtio-gpu-pci,id=ohgpu,xres=720,yres=1280 \
    -serial file:"$SERIAL" \
    -gdb tcp:127.0.0.1:"$GDBPORT" \
    -kernel "$KERNEL" -initrd "$RAMDISK" \
    -append "$APPEND" \
    $DISKS \
    -drive "if=none,id=ohd5,format=raw,file.locking=off,file=$USERDATA" -device virtio-blk-pci,drive=ohd5 \
    -nic user,model=virtio-net-pci
