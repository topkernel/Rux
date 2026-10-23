#!/bin/bash
# ww-cap3.sh — 20-point capacity probe (session 28).
# Verifies the gdbstub arms 20 simultaneous hw watchpoints (chain 15 +
# four timekeeper-neighborhood candidates) with firing at the deepest
# position (JIFFIES) before the chain set grows to 19. Bare boot of the
# frozen kernel + OH ramdisk, own port, own PIDs only.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PORT="${PORT:-12403}"
WATCH_SECS="${WATCH_SECS:-40}"
TAG="${TAG:-ww-cap3}"
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
echo "[cap3] qemu pid=$QPID port=$PORT"

sleep 8
python3 "$ROOT/run/ww-cap3-gdbscript.py" "$PORT" > "$CMDFILE"
echo "[cap3] cmd file watch count: $(grep -cE '^watch ' "$CMDFILE")"
gdb-multiarch -batch -x "$CMDFILE" > "$GDBLOG" 2>&1 &
GPID=$!
echo "[cap3] gdb pid=$GPID"

sleep "$WATCH_SECS"
kill "$GPID" 2>/dev/null || true
kill "$QPID" 2>/dev/null || true
wait "$GPID" 2>/dev/null || true
wait "$QPID" 2>/dev/null || true
trap - EXIT

echo "[cap3] arm confirmations: $(grep -cE '^Hardware watchpoint' "$GDBLOG")"
grep -E "Cannot insert|Error|out of" "$GDBLOG" | head -5 || true
grep "WW-CAP3-ARMED" "$GDBLOG" || true
echo "[cap3] quiet hits: $(grep -c "WW-CAP3-HIT" "$GDBLOG" || true)"
echo "[cap3] hot (JIFFIES) hits: $(grep -c "WW-CAP3-HOT" "$GDBLOG" || true)"
echo "[cap3] serial lines: $(wc -l < "$SERIAL")"
