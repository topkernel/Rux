#!/bin/bash
# s5-boot.sh — arch-s5 lane measurement boot (typed timer wheel A/B).
# Protocol copied from the perf-audit lane's pa-boot.sh (Tier-1 quiet
# syscost boot): q35 + KVM, smp2, 4G, OH image set from the central
# store (read-only, snapshot=on + file.locking=off), userdata = private
# per-tag copy of the tok109 lineage asset (source read from the
# perf-audit lane's run/ — read-only source, never written).
# Serial log + host-side stamper + window load monitor, same as pa.
#
# Usage: s5-boot.sh <tag> [kernel] [secs]
# Env: SMP=2 ACCEL=kvm APPEND_EXTRA="syscost=1 dfx=wwcensus"
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMGDIR="$ROOT/run/s5-oh-img"
TAG="${1:?usage: s5-boot.sh <tag> [kernel] [secs]}"
KERNEL="${2:-$ROOT/target/x86_64-unknown-none/debug/bzImage}"
SECS="${3:-780}"
RUN="$ROOT/run"
SERIAL="$RUN/s5-$TAG-serial.log"
STAMPED="$RUN/s5-$TAG-stamped.log"
MON="$RUN/s5-$TAG-mon.sock"
PIDFILE="$RUN/s5-$TAG-qemu.pid"
STAMPID="$RUN/s5-$TAG-stamp.pid"

# Userdata: private copy from the tok109 lineage asset (perf-audit
# lane, read-only source). ext4 has no reflink — a full 4G copy per
# tag, same lineage discipline as pa-boot.
UD_SRC="${UD_SRC:-/home/william/rux-agents/perf-audit/run/oh-userdata-tok109.img}"
UD="$RUN/s5-$TAG-userdata.img"
if [ ! -f "$UD" ]; then
    cp "$UD_SRC" "$UD"
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
APPEND="$APPEND ${APPEND_EXTRA:-syscost=1 dfx=wwcensus}"

DISKS="$(ro_drive 0 "$IMGDIR/updater.img")$(ro_drive 1 "$IMGDIR/system.img")$(ro_drive 2 "$IMGDIR/vendor.img")$(ro_drive 3 "$IMGDIR/sys_prod.img")$(ro_drive 4 "$IMGDIR/chip_prod.img")"

RAMDISK="$IMGDIR/ramdisk.img"
SMPN="${SMP:-2}"
ACCEL_ARGS="${ACCEL:-kvm}"

# Host-side timestamper (perf-audit's patstamp.py, read-only use).
python3 /home/william/rux-agents/perf-audit/run/patstamp.py "$SERIAL" "$STAMPED" "$((SECS + 120))" &
echo $! > "$STAMPID"

# Host-load window monitor (Tier-1 comparable-window discipline).
STOP="$RUN/s5-$TAG-window.stop"
rm -f "$STOP"
( while [ ! -f "$STOP" ]; do
    echo "$(date '+%F %T') load1=$(cut -d' ' -f1 /proc/loadavg) qemus=$(pgrep -c qemu-system || echo 0)"
    sleep 60
  done
  echo "$(date '+%F %T') window close: load1=$(cut -d' ' -f1 /proc/loadavg) qemus=$(pgrep -c qemu-system || echo 0)"
) > "$RUN/s5-$TAG-window.log" 2>&1 &
WINMON=$!

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
touch "$STOP"
sleep 2
if [ -f "$STAMPID" ]; then
    sp=$(cat "$STAMPID")
    kill "$sp" 2>/dev/null || true
    wait "$sp" 2>/dev/null || true
fi
wait $WINMON 2>/dev/null || true
echo "s5-boot: qemu exited rc=$rc after ${SECS}s (raw: $SERIAL, stamped: $STAMPED)"
