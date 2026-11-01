#!/bin/bash
# oh-boot-sc.sh — samgr-callback lane: OH fresh-lineage boot on current main.
# Copied from oh-svc's oh-boot-cook.sh (q35 KVM, virt, 4G, smp2), with:
#   - no -gdb (port 1239 belongs to oh-svc boots)
#   - userdata = this worktree's writable copy (NEVER the lineage master)
#   - EXTRA_APPEND honored before the dfx flags so boots can add probes
#
# Usage: oh-boot-sc.sh <tag> [userdata-img] [kernel]
# Env: SECS (default 1200), APPEND_EXTRA (extra cmdline tokens)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMGDIR="$ROOT/run/oh-img"
TAG="${1:?usage: oh-boot-sc.sh <tag> [userdata-img] [kernel]}"
USERDATA="${2:-$ROOT/run/userdata-sc41.img}"
KERNEL="${3:-$ROOT/target/x86_64-unknown-none/debug/bzImage}"
RUN="$ROOT/run"
SERIAL="$RUN/oh-$TAG-serial.log"
MON="$RUN/oh-$TAG-mon.sock"

rm -f "$SERIAL" "$MON"

ro_drive() { # idx file — snapshot=on
    printf ' -drive file=%s,if=none,id=ohd%d,format=raw,file.locking=off,snapshot=on -device virtio-blk-pci,drive=ohd%d' "$1" "$2" "$2"
}

APPEND="ip=dhcp loglevel=7 console=ttyS0,115200"
APPEND="$APPEND init=init root=/dev/ram0 rw"
APPEND="$APPEND ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio"
APPEND="$APPEND ohos.boot.sn=01234567890"
APPEND="$APPEND ohos.required_mount.system=/dev/block/vdb@/usr@ext4@ro,barrier=1@wait,required"
APPEND="$APPEND ohos.required_mount.vendor=/dev/block/vdc@/vendor@ext4@ro,barrier=1@wait,required"
APPEND="$APPEND ${APPEND_EXTRA:-}"

# No global -snapshot: userdata must persist. Other drives snapshot=on.
DISKS="$(ro_drive "$IMGDIR/updater.img" 0)$(ro_drive "$IMGDIR/system.img" 1)"
DISKS="$DISKS$(ro_drive "$IMGDIR/vendor.img" 2)$(ro_drive "$IMGDIR/sys_prod.img" 3)"
DISKS="$DISKS$(ro_drive "$IMGDIR/chip_prod.img" 4)"
DISKS="$DISKS -drive file=$USERDATA,if=none,id=ohd5,format=raw,file.locking=off -device virtio-blk-pci,drive=ohd5"

RAMDISK="$IMGDIR/ramdisk-x64.img"
[ -f "$RAMDISK" ] || RAMDISK="$IMGDIR/ramdisk.img"

SECS="${SECS:-1200}"

# shellcheck disable=SC2086
timeout --signal=TERM --kill-after=10 "${SECS}s" qemu-system-x86_64 \
    -M q35 -accel kvm -cpu max -m 4G -smp 2 \
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

echo "oh-boot-sc: finished after ${SECS}s (log: $SERIAL)"
