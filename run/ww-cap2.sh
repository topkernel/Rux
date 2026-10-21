#!/bin/bash
# ww-cap2.sh — 15-point capacity probe (session 13).
# Verifies the EXPANDED chain watchset (15 simultaneous hw watchpoints)
# arms completely on this QEMU gdbstub before r24+ rides it. Bare boot
# of the frozen kernel + OH ramdisk, own port, own PIDs only.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PORT="${PORT:-12398}"
WATCH_SECS="${WATCH_SECS:-40}"
TAG="${TAG:-ww-cap2}"
SERIAL="$ROOT/run/$TAG-serial.log"
GDBLOG="$ROOT/run/$TAG-gdb.log"
CMDFILE="$ROOT/run/$TAG-gdb.cmd"
RAMDISK="/home/william/rux-agents/oh-display/run/oh-img/ramdisk-x64.img"

rm -f "$SERIAL" "$GDBLOG" "$CMDFILE"

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
echo "[cap2] qemu pid=$QPID port=$PORT"

sleep 8
python3 "$ROOT/run/ww-gdbscript.py" "$PORT" > "$CMDFILE"
echo "[cap2] cmd file watch count: $(grep -cE '^watch ' "$CMDFILE")"
gdb-multiarch -batch -x "$CMDFILE" > "$GDBLOG" 2>&1 &
GPID=$!
echo "[cap2] gdb pid=$GPID"

sleep "$WATCH_SECS"
kill "$GPID" 2>/dev/null || true
kill "$QPID" 2>/dev/null || true
wait "$GPID" 2>/dev/null || true
wait "$QPID" 2>/dev/null || true
trap - EXIT

echo "[cap2] arm confirmations: $(grep -cE '^Hardware watchpoint' "$GDBLOG")"
grep -E "Cannot insert|Error|out of" "$GDBLOG" | head -5 || true
grep "WW-ARMED" "$GDBLOG" || true
echo "[cap2] hits: $(grep -c "WW-HIT" "$GDBLOG" || true)"
echo "[cap2] serial lines: $(wc -l < "$SERIAL")"
