#!/bin/bash
# ww-cap.sh — one-shot watchpoint capacity probe (session 10 experiment).
# Bare boot of the FROZEN hunt kernel + OH ramdisk (no disks), gdbstub on a
# private port, 10 hw watchpoints for WATCH_SECS, then teardown.
# Own PID discipline: only the QEMU/gdb started here are killed.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PORT="${PORT:-12399}"
WATCH_SECS="${WATCH_SECS:-30}"
TAG="ww-cap1"
SERIAL="$ROOT/run/$TAG-serial.log"
GDBLOG="$ROOT/run/$TAG-gdb.log"
RAMDISK="/home/william/rux-agents/oh-display/run/oh-img/ramdisk-x64.img"

rm -f "$SERIAL" "$GDBLOG"

qemu-system-x86_64 \
    -M q35 -accel tcg,thread=multi -cpu max -m 2G -smp 2 \
    -gdb tcp:127.0.0.1:"$PORT" \
    -display none -vga none \
    -serial file:"$SERIAL" \
    -kernel "$ROOT/run/ww-hunt-frozen-bzImage" \
    -initrd "$RAMDISK" \
    -append "console=ttyS0,115200 loglevel=4" &
QPID=$!
trap 'kill $QPID 2>/dev/null || true; wait $QPID 2>/dev/null || true' EXIT
echo "[cap] qemu pid=$QPID port=$PORT"

sleep 8
python3 "$ROOT/run/ww-cap-gdbscript.py" "$PORT" > "$ROOT/run/$TAG-gdb.cmd"
gdb-multiarch -batch -x "$ROOT/run/$TAG-gdb.cmd" > "$GDBLOG" 2>&1 &
GPID=$!
echo "[cap] gdb pid=$GPID armed"

sleep "$WATCH_SECS"
kill "$GPID" 2>/dev/null || true
kill "$QPID" 2>/dev/null || true
wait "$GPID" 2>/dev/null || true
wait "$QPID" 2>/dev/null || true
trap - EXIT

echo "[cap] done. hits per point:"
grep -oE "WW-CAP-HIT [A-Z_]+" "$GDBLOG" | sort | uniq -c || echo "(none)"
echo "[cap] arm status:"
grep -E "WW-CAP-ARMED|watchpoint|Cannot insert" "$GDBLOG" | head -15
