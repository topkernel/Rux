#!/bin/bash
# ww-mmaps-repro.sh — reproduce the loader file-LOAD fd/offset loss on a
# live OH boot (mm-lane handoff from launcher Session L).
#
# Boots the FROZEN hunt kernel (current branch state) on the OH image
# (all shared drives -snapshot + file.locking=off) with the serial
# console on a unix socket so the OH console shell can be driven; the
# caller then asks for /proc/<pid>/maps of processes with 0x3fbf
# (prelinked shared-lib) mappings and inspects fd/offset fidelity.
#
# Own PID discipline: only the QEMU started here is killed (via $QPID).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMGDIR="/home/william/rux-agents/oh-display/run/oh-img"
TAG="ww-mmaps1"
SERIAL="$ROOT/run/$TAG-serial.log"
SOCK="$ROOT/run/$TAG-ser.sock"
RAMDISK="$IMGDIR/ramdisk-x64.img"
KERNEL="${KERNEL:-$ROOT/run/ww-hunt-frozen-bzImage}"
SECS="${SECS:-600}"

rm -f "$SERIAL" "$SOCK"

qemu_drive() { # idx file
    local idx=$1 file=$2
    [ -f "$file" ] || return 0
    printf ' -drive file=%s,if=none,id=ohd%d,format=raw,file.locking=off -device virtio-blk-pci,drive=ohd%d' "$file" "$idx" "$idx"
}

USERDATA="$IMGDIR/userdata-tok.img"
[ -f "$USERDATA" ] || USERDATA="$IMGDIR/userdata-rw.img"
[ -f "$USERDATA" ] || USERDATA="$IMGDIR/userdata.img"

APPEND="ip=dhcp loglevel=4 console=ttyS0,115200"
APPEND="$APPEND init=init root=/dev/ram0 rw"
APPEND="$APPEND ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio"
APPEND="$APPEND ohos.boot.sn=01234567890"
APPEND="$APPEND ohos.required_mount.system=/dev/vdb@/usr@ext4@ro,barrier=1@wait,required"
APPEND="$APPEND ohos.required_mount.vendor=/dev/vdc@/vendor@ext4@ro,barrier=1@wait,required"

DISKS="$(qemu_drive 0 "$IMGDIR/updater.img")$(qemu_drive 1 "$IMGDIR/system.img")$(qemu_drive 2 "$IMGDIR/vendor.img")$(qemu_drive 3 "$IMGDIR/sys_prod.img")$(qemu_drive 4 "$IMGDIR/chip_prod.img")$(qemu_drive 5 "$USERDATA")"

timeout --signal=TERM --kill-after=10 "${SECS}s" qemu-system-x86_64 \
    -M q35 -accel kvm -cpu max -m 4G -smp 2 -snapshot \
    -display none -vga none \
    -device virtio-gpu-pci,id=ohgpu,xres=720,yres=1280 \
    -chardev socket,id=ser0,path="$SOCK",logfile="$SERIAL",server=on,wait=off \
    -serial chardev:ser0 \
    -monitor none \
    -kernel "$KERNEL" -initrd "$RAMDISK" \
    -append "$APPEND" \
    $DISKS \
    -nic user,model=virtio-net-pci \
    -device virtio-keyboard-pci -device virtio-tablet-pci &
QPID=$!
echo "[repro] qemu pid=$QPID sock=$SOCK"
echo $QPID > "$ROOT/run/$TAG.pid"
wait $QPID || true
