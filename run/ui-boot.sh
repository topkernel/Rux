#!/bin/bash
# ui-boot.sh — Agent A (ui-sprint) OH boot: same machine/device/cmdline as
# oh-boot-tmp.sh, but the userdata drive is my private reflink copy with
# writes PERSISTED (no snapshot on that one drive) so /data/log/hilog
# files can be extracted after the boot with debugfs. The five shared
# read-only images keep snapshot=on (never write oh-robot's files).
#
# Usage: ui-boot.sh <tag> [kernel]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMGDIR="$ROOT/run/oh-img"
TAG="${1:?usage: ui-boot.sh <tag> [kernel]}"
KERNEL="${2:-$ROOT/target/x86_64-unknown-none/debug/bzImage}"
RUN="$ROOT/run"
SERIAL="$RUN/oh-$TAG-serial.log"
MON="$RUN/oh-$TAG-mon.sock"

rm -f "$SERIAL" "$MON"

ro_drive() { # idx file
    local idx=$1 file=$2
    [ -f "$file" ] || return 0
    printf ' -drive file=%s,if=none,id=ohd%d,format=raw,file.locking=off,snapshot=on -device virtio-blk-pci,drive=ohd%d' "$file" "$idx" "$idx"
}

USERDATA="${USERDATA:-$IMGDIR/userdata-tok.img}"

APPEND="ip=dhcp loglevel=${LOGLEVEL:-7} console=ttyS0,115200"
APPEND="$APPEND init=init root=/dev/ram0 rw"
APPEND="$APPEND ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio"
APPEND="$APPEND ohos.boot.sn=01234567890"
APPEND="$APPEND ohos.required_mount.system=/dev/block/vdb@/usr@ext4@ro,barrier=1@wait,required"
APPEND="$APPEND ohos.required_mount.vendor=/dev/block/vdc@/vendor@ext4@ro,barrier=1@wait,required"
APPEND="$APPEND ${EXTRA_APPEND:-}"

# SYSTEMIMG: override the system drive (e.g. system-paint.img whose
# /bin/bootanimation is the egl_painter seat). Defaults to the shared
# oh-robot system.img (snapshot=on, pristine).
SYSTEMIMG="${SYSTEMIMG:-$IMGDIR/system.img}"

DISKS="$(ro_drive 0 "$IMGDIR/updater.img")$(ro_drive 1 "$SYSTEMIMG")$(ro_drive 2 "${VENDORIMG:-$IMGDIR/vendor.img}")$(ro_drive 3 "$IMGDIR/sys_prod.img")$(ro_drive 4 "$IMGDIR/chip_prod.img")"
# userdata: writable, file.locking=off (shared-machine rule), NO snapshot.
DISKS="$DISKS -drive file=$USERDATA,if=none,id=ohd5,format=raw,file.locking=off -device virtio-blk-pci,drive=ohd5"

RAMDISK="$IMGDIR/ramdisk-x64.img"
[ -f "$RAMDISK" ] || RAMDISK="$IMGDIR/ramdisk.img"

SMPN="${SMP:-2}"
ACCEL_ARGS="${ACCEL:-kvm}"
SECS="${SECS:-3600}"

# SERIAL_SOCK=1: bidirectional serial on a unix socket (with logfile)
# so the OH console service can be woken by input and a shell driven
# interactively (hilog -x etc). Falls back to -serial file: otherwise.
SERIAL_ARGS="-serial file:$SERIAL"
if [ "${SERIAL_SOCK:-0}" = "1" ]; then
    SERIAL_ARGS="-chardev socket,id=ser0,path=$RUN/oh-$TAG-ser.sock,logfile=$SERIAL,server=on,wait=off -serial chardev:ser0"
fi

# shellcheck disable=SC2086
exec timeout --signal=TERM --kill-after=10 "${SECS}s" qemu-system-x86_64 \
    -M q35 -accel "$ACCEL_ARGS" -cpu max -m 4G -smp "$SMPN" \
    $([ "${GDB:-0}" = "1" ] && echo -s) \
    -display none \
    -vga none \
    -device virtio-gpu-pci,id=ohgpu,xres=720,yres=1280 \
    $SERIAL_ARGS \
    -monitor unix:"$MON",server=on,wait=off \
    -kernel "$KERNEL" -initrd "$RAMDISK" \
    -append "$APPEND" \
    $DISKS \
    -nic user,model=virtio-net-pci \
    -device virtio-keyboard-pci -device virtio-tablet-pci
