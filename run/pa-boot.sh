#!/bin/bash
# pa-boot.sh — perf-audit measurement boot: quiet OH boot with
#   - syscost=1 kernel instrumentation (syscall cost table + tick-mode
#     accounting + kernel-rip histogram, dumped every 60 s), and
#   - host-side serial timestamping (run/patstamp.py) for the
#     wall-clock phase timeline.
#
# Modeled on oh-boot-w.sh. Shared read-only images ride snapshot=on +
# file.locking=off; userdata is a private reflink copy (never the
# shared original).
#
# Usage: pa-boot.sh <tag> [kernel] [secs]
# Env: SMP=2 ACCEL=kvm APPEND_EXTRA="syscost=1 ..."
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMGDIR="$ROOT/run/oh-img"
TAG="${1:?usage: pa-boot.sh <tag> [kernel] [secs]}"
KERNEL="${2:-$ROOT/target/x86_64-unknown-none/debug/bzImage}"
SECS="${3:-1560}"
RUN="$ROOT/run"
SERIAL="$RUN/pa-$TAG-serial.log"
STAMPED="$RUN/pa-$TAG-stamped.log"
MON="$RUN/pa-$TAG-mon.sock"
PIDFILE="$RUN/pa-$TAG-qemu.pid"
STAMPID="$RUN/pa-$TAG-stamp.pid"

# Userdata: private reflink copy of the tok image (first use only).
UD_SRC="${UD_SRC:-$IMGDIR/userdata-tok.img}"
UD="$RUN/pa-$TAG-userdata.img"
if [ ! -f "$UD" ]; then
    cp --reflink=auto "$UD_SRC" "$UD"
fi

rm -f "$SERIAL" "$STAMPED" "$MON" "$PIDFILE" "$SERIAL.done"

ro_drive() { # idx file (shared, read-only overlay)
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
APPEND="$APPEND ${APPEND_EXTRA:-syscost=1}"

DISKS="$(ro_drive 0 "$IMGDIR/updater.img")$(ro_drive 1 "$IMGDIR/system.img")$(ro_drive 2 "$IMGDIR/vendor.img")$(ro_drive 3 "$IMGDIR/sys_prod.img")$(ro_drive 4 "$IMGDIR/chip_prod.img")"

RAMDISK="$IMGDIR/ramdisk-x64.img"
[ -f "$RAMDISK" ] || RAMDISK="$IMGDIR/ramdisk.img"

SMPN="${SMP:-2}"
ACCEL_ARGS="${ACCEL:-kvm}"

# Host-side timestamper (background; stopped at the end).
python3 "$RUN/patstamp.py" "$SERIAL" "$STAMPED" "$((SECS + 120))" &
echo $! > "$STAMPID"

# `|| rc=$?` keeps `set -e` from aborting at the horizon kill: timeout
# returns 124 when it TERMs QEMU at SECS, and the tail below (the .done
# marker + stamper kill) must still run — pa30smoke caught the script
# dying at the qemu line, leaving the stamper alive and no .done file.
rc=0
# shellcheck disable=SC2086
timeout --signal=TERM --kill-after=10 "${SECS}s" qemu-system-x86_64 \
    -M q35 -accel "$ACCEL_ARGS" -cpu max -m 4096M -smp "$SMPN" \
    -display none \
    -vga none \
    -device virtio-gpu-pci,id=ohgpu,xres=720,yres=1280 \
    -serial file:"$SERIAL" \
    -monitor unix:"$MON",server=on,wait=off \
    -pidfile "$PIDFILE" \
    -kernel "$KERNEL" -initrd "$RAMDISK" \
    -append "$APPEND" \
    $DISKS \
    -drive "if=none,id=ohd5,format=raw,file.locking=off,file=$UD" -device virtio-blk-pci,drive=ohd5 \
    -nic user,model=virtio-net-pci \
    -device virtio-keyboard-pci -device virtio-tablet-pci \
    || rc=$?

touch "$SERIAL.done"
sleep 2
if [ -f "$STAMPID" ]; then
    sp=$(cat "$STAMPID")
    kill "$sp" 2>/dev/null || true
    wait "$sp" 2>/dev/null || true
fi
echo "pa-boot: qemu exited rc=$rc after ${SECS}s (raw: $SERIAL, stamped: $STAMPED)"
