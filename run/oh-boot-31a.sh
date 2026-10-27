#!/bin/bash
# oh-boot-31a.sh — OH patched-image boot for leg-31A (kernel-side lane).
# Same recipe as leg-30A's oh-boot-30a.sh: the shared oh-img-p directory is
# referenced READ-ONLY (absolute path into the samgr-callback worktree —
# never written from here; every shared drive gets snapshot=on +
# file.locking=off) and the userdata is a fresh private copy of the scgold
# master made into THIS worktree under a per-tag name (30A mishap lesson:
# per-tag userdata paths so a retried launch can never truncate another
# boot's guest storage).
#
# Default instrumentation for the leg-31A draw series (bake into APPEND so
# every draw in the series is comparable): dfx=txnhunt,wwcensus fsw=1.
#
# Usage: oh-boot-31a.sh <tag> [kernel]
# Env: SMP=2 SECS=2700 EXTRA_APPEND=... HDCPORT=1223x
set -euo pipefail

IMGDIR="/home/william/rux-agents/samgr-callback/run/oh-img-p"
SCGOLD="/home/william/rux-agents/samgr-callback/run/userdata-scgold.img"
TAG="${1:?usage: oh-boot-31a.sh <tag> [kernel]}"
KERNEL="${2:-/home/william/rux-agents/oh-kernel/target/x86_64-unknown-none/debug/bzImage}"
RUN="/home/william/rux-agents/oh-kernel/run"
USERDATA="$RUN/sc31a-$TAG-userdata.img"
SERIAL="$RUN/oh31a-$TAG-serial.log"
MON="$RUN/oh31a-$TAG-mon.sock"
PIDFILE="$RUN/oh31a-$TAG-qemu.pid"

rm -f "$SERIAL" "$MON" "$PIDFILE"
cp --reflink=auto "$SCGOLD" "$USERDATA"

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
APPEND="$APPEND dfx=txnhunt,wwcensus fsw=1 ${EXTRA_APPEND:-}"

DISKS="$(qemu_drive 0 "$IMGDIR/updater.img")$(qemu_drive 1 "$IMGDIR/system.img")$(qemu_drive 2 "$IMGDIR/vendor.img")$(qemu_drive 3 "$IMGDIR/sys_prod.img")$(qemu_drive 4 "$IMGDIR/chip_prod.img")"

RAMDISK="$IMGDIR/ramdisk-x64.img"
[ -f "$RAMDISK" ] || RAMDISK="$IMGDIR/ramdisk.img"

SMPN="${SMP:-2}"
SECS="${SECS:-2700}"

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
    -nic user,model=virtio-net-pci,hostfwd=tcp:127.0.0.1:${HDCPORT:-12231}-:10178 \
    -device virtio-keyboard-pci -device virtio-tablet-pci

echo "oh-boot-31a: finished after ${SECS}s (log: $SERIAL)"
