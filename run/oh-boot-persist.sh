#!/bin/bash
# oh-boot-persist.sh — OH boot with a WRITABLE userdata (ui-launcher
# timer/dfx lane). Same command as run/oh-boot.sh except:
#
#   - no global -snapshot: the userdata drive is written for real, so
#     hilogd's /data/log/hilog files persist for post-boot debugfs
#     extraction (the "hilog | grep ..." flow without a guest shell:
#     debugfs -R "cat <path>" on the userdata image afterwards);
#   - all READ-ONLY disks (system/vendor/sys_prod/chip_prod/updater)
#     still ride snapshot=on so the shared originals stay pristine;
#   - USERDATA defaults to this worktree's reflink copy of the tok
#     image — never the shared oh-display original.
#
# Usage: oh-boot-persist.sh <tag> [kernel]
# Env: SMP=2 ACCEL=kvm SECS=1500 EXTRA_APPEND="dfx=monowatch,ledger ..."
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMGDIR="$ROOT/run/oh-img"
TAG="${1:?usage: oh-boot-persist.sh <tag> [kernel]}"
KERNEL="${2:-$ROOT/target/x86_64-unknown-none/debug/bzImage}"
RUN="$ROOT/run"
SERIAL="$RUN/oh-$TAG-serial.log"
MON="$RUN/oh-$TAG-mon.sock"

rm -f "$SERIAL" "$MON"

# Read-only disks: snapshot=on (shared originals pristine even without
# a global -snapshot flag).
ro_drive() { # idx file
    local idx=$1 file=$2
    [ -f "$file" ] || return 0
    printf ' -drive file=%s,if=none,id=ohd%d,format=raw,file.locking=off,snapshot=on -device virtio-blk-pci,drive=ohd%d' "$file" "$idx" "$idx"
}

# Writable userdata (reflink copy in THIS worktree).
USERDATA="${USERDATA:-$IMGDIR/userdata-tok.img}"
[ -f "$USERDATA" ] || { echo "missing $USERDATA" >&2; exit 1; }

# loglevel 7: verdict lines ride the /dev/kmsg pr_info passthrough
# (comparability with b154/b155 reference boots).
APPEND="ip=dhcp loglevel=7 console=ttyS0,115200"
APPEND="$APPEND init=init root=/dev/ram0 rw"
APPEND="$APPEND ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio"
APPEND="$APPEND ohos.boot.sn=01234567890"
APPEND="$APPEND ohos.required_mount.system=/dev/block/vdb@/usr@ext4@ro,barrier=1@wait,required"
APPEND="$APPEND ohos.required_mount.vendor=/dev/block/vdc@/vendor@ext4@ro,barrier=1@wait,required"
APPEND="$APPEND ${EXTRA_APPEND:-}"

DISKS="$(ro_drive 0 "$IMGDIR/updater.img")$(ro_drive 1 "$IMGDIR/system.img")$(ro_drive 2 "$IMGDIR/vendor.img")$(ro_drive 3 "$IMGDIR/sys_prod.img")$(ro_drive 4 "$IMGDIR/chip_prod.img")"
DISKS="$DISKS -drive file=$USERDATA,if=none,id=ohd5,format=raw,file.locking=off -device virtio-blk-pci,drive=ohd5"

RAMDISK="$IMGDIR/ramdisk-x64.img"
[ -f "$RAMDISK" ] || RAMDISK="$IMGDIR/ramdisk.img"

SMPN="${SMP:-2}"
ACCEL_ARGS="${ACCEL:-kvm}"
SECS="${SECS:-1500}"

# shellcheck disable=SC2086
timeout --signal=TERM --kill-after=10 "${SECS}s" qemu-system-x86_64 \
    -M q35 -accel "$ACCEL_ARGS" -cpu max -m 4G -smp "$SMPN" \
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

echo "oh-boot-persist: finished after ${SECS}s (log: $SERIAL)"
