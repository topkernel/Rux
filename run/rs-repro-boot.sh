#!/bin/bash
# rs-repro-boot.sh — render_service deterministic-crash reproduction boot
# (mm lane, relayed from launcher Session O). Mirrors the launcher's
# oh-boot-usbless.sh shape: their OH images read-only (snapshot=on +
# file.locking=off per drive), a PRIVATE userdata copy (written for
# real), THIS worktree's kernel. Usage: rs-repro-boot.sh <tag> [kernel]
# Env: SMP=2 SECS=1200 EXTRA_APPEND=...
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMGDIR="/home/william/rux-agents/samgr-callback/run/oh-img-p"
USERDATA="$ROOT/run/rs-userdata.img"
KERNEL="${2:-$ROOT/target/x86_64-unknown-none/debug/bzImage}"
RUN="$ROOT/run"
TAG="${1:?usage: rs-repro-boot.sh <tag> [kernel]}"
SERIAL="$RUN/rs-$TAG-serial.log"
MON="$RUN/rs-$TAG-mon.sock"
PIDFILE="$RUN/rs-$TAG-qemu.pid"

rm -f "$SERIAL" "$MON" "$PIDFILE"
[ -f "$USERDATA" ] || cp "$IMGDIR/../userdata-scgold.img" "$USERDATA"

qemu_drive() { # idx file (shared, read-only overlay)
    local idx=$1 file=$2
    [ -f "$file" ] || return 0
    printf ' -drive file=%s,if=none,id=ohd%d,format=raw,file.locking=off,snapshot=on -device virtio-blk-pci,drive=ohd%d' "$file" "$idx" "$idx"
}

APPEND="ip=dhcp loglevel=4 console=ttyS0,115200"
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
SECS="${SECS:-1200}"
ACCEL="${ACCEL:-kvm}"

# shellcheck disable=SC2086
timeout --signal=TERM --kill-after=10 "${SECS}s" qemu-system-x86_64 \
    -M q35 -accel "$ACCEL" -cpu max -m 4096M -smp "$SMPN" \
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
    -nic user,model=virtio-net-pci \
    -device virtio-keyboard-pci -device virtio-tablet-pci
