#!/bin/bash
# pbrux-boot.sh — ONE Rux-side pbench probe boot (perf-report.md 8.4/8.6
# run day, Rux half; the Linux half is banked in 8.8 via pbref-boot.sh).
# Boots the OH image on a POST-UI main-class RUX kernel with the private
# pbench-injected system image (pbref-inject.sh — same recipe both sides:
# key_enable slot, start conditioned on bootevent.launcher.ready=true)
# and a private per-run userdata copy (no snapshot: probe writes persist).
#
# Machine class matched to the pa series AND the ref boots (8.4):
#   q35 / KVM / -cpu max / SMP2 / 4G.
# PLAIN kernel (no syscost) — probe numbers must stay unperturbed; the
# syscost refresh for table 8b comes from the separate quiet boot
# (8.6 step 2, pa-boot.sh), never from this script.
#
# The probe starts only when the UI milestone fires (the condition IS
# the milestone) and powers the VM off when done; this script waits for
# QEMU exit (timeout backstop), then extracts
# /local/pbench/pbench-results.txt from the userdata copy via debugfs.
#
# Usage: pbrux-boot.sh <tag> [kernel]      (artifacts: run/pbrux-<tag>/)
# Env:   SECS timeout backstop (default 2700 — backstop only)
#        SMP=2 ACCEL=kvm APPEND_EXTRA=""   (keep EMPTY for probe runs)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
IMGDIR="$ROOT/run/oh-img"
TAG="${1:?usage: pbrux-boot.sh <tag> [kernel]}"
KERNEL="${2:-$ROOT/target/x86_64-unknown-none/debug/bzImage}"
SECS="${SECS:-2700}"
SYS="$ROOT/run/pbrux-system.img"
UD="$ROOT/run/pbrux-$TAG-userdata.img"
RUN="$ROOT/run/pbrux-$TAG"

[ -f "$KERNEL" ] || { echo "kernel missing: $KERNEL" >&2; exit 1; }

# Private injected system image (idempotent; reflink copy of the shared
# system.img — shared images are never written).
"$ROOT/run/pbref-inject.sh" "$SYS" >/dev/null

mkdir -p "$RUN"
if [ ! -f "$UD" ]; then
    cp --reflink=auto "$IMGDIR/userdata-tok.img" "$UD"
fi

SERIAL="$RUN/serial.log"
MON="$RUN/mon.sock"
PIDFILE="$RUN/qemu.pid"
rm -f "$SERIAL" "$MON" "$PIDFILE"

ro_drive() { # idx file (snapshot=on, file.locking=off)
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
APPEND="$APPEND ${APPEND_EXTRA:-}"

# system drive = the injected private copy (ro-mounted anyway); userdata
# = private copy WITHOUT snapshot (probe writes must persist).
DISKS="$(ro_drive 0 "$IMGDIR/updater.img")$(ro_drive 1 "$SYS")$(ro_drive 2 "$IMGDIR/vendor.img")$(ro_drive 3 "$IMGDIR/sys_prod.img")$(ro_drive 4 "$IMGDIR/chip_prod.img")"

RAMDISK="$IMGDIR/ramdisk-x64.img"
[ -f "$RAMDISK" ] || RAMDISK="$IMGDIR/ramdisk.img"

SMPN="${SMP:-2}"
ACCEL_ARGS="${ACCEL:-kvm}"

foreign=$(pgrep -c qemu-system || true)
echo "[boot] pbrux-$TAG start $(date +%H:%M:%S) kernel=$KERNEL machine=q35/$ACCEL_ARGS/-cpu max/SMP$SMPN/4G foreign_qemus=${foreign:-0}"

rc=0
# shellcheck disable=SC2086
timeout --signal=TERM --kill-after=15 "${SECS}s" qemu-system-x86_64 \
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

echo "[boot] pbrux-$TAG qemu rc=$rc after at-most ${SECS}s ($(date +%H:%M:%S))"

# Milestone + probe evidence from serial (live channel). The SET
# evidence is param_service's "Handle set param ... key:
# bootevent.launcher.ready" (ref-r1 format); init's "not set" warning
# matches the plain substring, so SET gets its own check.
grep -E "launcher\.ready|bootanimation\.finished|composer_host\.ready|OnForeground|ArkUI" "$SERIAL" 2>/dev/null | tail -8 > "$RUN/milestone-lines.txt" || true
grep -q "key: bootevent.launcher.ready" "$SERIAL" 2>/dev/null \
    && echo "[boot] MILESTONE: launcher.ready SET in serial" \
    || echo "[boot] milestone NOT reached (no launcher.ready SET)" >&2
grep "PBENCH " "$SERIAL" 2>/dev/null > "$RUN/pbench-serial-lines.txt" || true
grep -q "PBENCH DONE rc=0" "$SERIAL" 2>/dev/null && echo "[boot] DONE marker in serial" || echo "[boot] no DONE marker in serial" >&2

# Results: file channel (authoritative) from the private userdata copy.
if debugfs -R "dump /local/pbench/pbench-results.txt $RUN/results.txt" "$UD" >/dev/null 2>&1; then
    echo "[boot] results file extracted: $RUN/results.txt"
    grep -c "^PBENCH" "$RUN/results.txt" || true
else
    echo "[boot] NO results file in userdata (probe did not reach file output)" >&2
fi
exit 0
