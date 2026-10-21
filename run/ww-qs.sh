#!/bin/bash
# ww-qs.sh — storm-quietness probe (session 13 experiment).
# Full OH disk boot of the FROZEN hunt kernel (same disk set and boot
# conditions as the ww-s2 chain: q35/tcg-multi/smp2/4G, -snapshot,
# file.locking=off) on a private gdbstub port, watching ONLY the 5
# storm-unverified candidate statics. Count hits after WATCH_SECS of
# storm; quiet candidates are chain-expandable.
# Own-PID discipline: only the QEMU/gdb started here are killed.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PORT="${PORT:-12398}"
WATCH_SECS="${WATCH_SECS:-600}"
TAG="${TAG:-ww-qs1}"
SERIAL="$ROOT/run/$TAG-serial.log"
GDBLOG="$ROOT/run/$TAG-gdb.log"
RAMDISK="/home/william/rux-agents/oh-display/run/oh-img/ramdisk-x64.img"
IMG="/home/william/rux-agents/oh-display/run/oh-img"

rm -f "$SERIAL" "$GDBLOG"

qemu-system-x86_64 \
    -M q35 -accel tcg,thread=multi -cpu max -m 4G -smp 2 \
    -snapshot \
    -gdb tcp:127.0.0.1:"$PORT" \
    -display none -vga none \
    -device virtio-gpu-pci,id=ohgpu,xres=720,yres=1280 \
    -serial file:"$SERIAL" \
    -kernel "$ROOT/run/ww-hunt-frozen-bzImage" \
    -initrd "$RAMDISK" \
    -append "ip=dhcp loglevel=7 console=ttyS0,115200 init=init root=/dev/ram0 rw ohos.boot.hardware=virt default_boot_device=10007000.virtio_mmio ohos.boot.sn=01234567890 ohos.required_mount.system=/dev/vdb@/usr@ext4@ro,barrier=1@wait,required ohos.required_mount.vendor=/dev/vdc@/vendor@ext4@ro,barrier=1@wait,required" \
    -drive file="$IMG/updater.img",if=none,id=ohd0,format=raw,file.locking=off \
    -device virtio-blk-pci,drive=ohd0 \
    -drive file="$IMG/system.img",if=none,id=ohd1,format=raw,file.locking=off \
    -device virtio-blk-pci,drive=ohd1 \
    -drive file="$IMG/vendor.img",if=none,id=ohd2,format=raw,file.locking=off \
    -device virtio-blk-pci,drive=ohd2 \
    -drive file="$IMG/sys_prod.img",if=none,id=ohd3,format=raw,file.locking=off \
    -device virtio-blk-pci,drive=ohd3 \
    -drive file="$IMG/chip_prod.img",if=none,id=ohd4,format=raw,file.locking=off \
    -device virtio-blk-pci,drive=ohd4 \
    -drive file="$IMG/userdata-tok.img",if=none,id=ohd5,format=raw,file.locking=off \
    -device virtio-blk-pci,drive=ohd5 \
    -nic user,model=virtio-net-pci \
    -device virtio-keyboard-pci -device virtio-tablet-pci &
QPID=$!
trap 'kill $QPID 2>/dev/null || true; wait $QPID 2>/dev/null || true' EXIT
echo "[qs] qemu pid=$QPID port=$PORT tag=$TAG"

# Arm after early boot (same delay as the chain: init-time writers of
# the candidates land before this point).
sleep 20
python3 "$ROOT/run/ww-qs-gdbscript.py" "$PORT" > "$ROOT/run/$TAG-gdb.cmd"
gdb-multiarch -batch -x "$ROOT/run/$TAG-gdb.cmd" > "$GDBLOG" 2>&1 &
GPID=$!
echo "[qs] gdb pid=$GPID armed (5 candidates)"

sleep "$WATCH_SECS"
kill "$GPID" 2>/dev/null || true
kill "$QPID" 2>/dev/null || true
wait "$GPID" 2>/dev/null || true
wait "$QPID" 2>/dev/null || true
trap - EXIT

echo "[qs] done. hits per candidate:"
grep -oE "WW-HIT [A-Z_]+" "$GDBLOG" | sort | uniq -c || echo "(none)"
echo "[qs] arm status:"
grep -E "WW-ARMED|Cannot insert|Error" "$GDBLOG" | head -5
echo "[qs] serial lines: $(wc -l < "$SERIAL")"
