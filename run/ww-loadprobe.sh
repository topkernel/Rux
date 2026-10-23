#!/bin/bash
# ww-loadprobe.sh — s31 load-diversification hunt round (probe shape of
# ww-hunt.py, against the sdc-cycle image via ww-loadboot.sh).
#
# Full capture stack, same as a chain round: frozen kernel + dfx=wildwatch
# (kernel RO tripwire over the guard cluster + 1s canary) + the FULL
# 19-point gdb watchset (ww-gdbscript.py). Any WW-HIT (gdb) or
# WILDWATCH-CANARY / scribble-heal / KERNPANIC (serial) is a CAPTURE.
#
# Usage: ww-loadprobe.sh <tag> [secs] [port]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUN="$ROOT/run"
TAG="${1:-wwload}"
SECSN="${2:-930}"
PORT="${3:-12381}"

SERIAL="$RUN/${TAG}-serial.log"
GDBLOG="$RUN/${TAG}-gdb.log"
GDBCMD="$RUN/${TAG}-gdb.cmd"

export TAG GDBPORT=$PORT SECS=$SECSN
export EXTRA_APPEND="dfx=wildwatch"

qemu_pid=""
gdb_pid=""
cleanup() {
    if [ -n "$gdb_pid" ] && kill -0 "$gdb_pid" 2>/dev/null; then
        kill "$gdb_pid" 2>/dev/null || true
    fi
    if [ -n "$qemu_pid" ] && kill -0 "$qemu_pid" 2>/dev/null; then
        kill "$qemu_pid" 2>/dev/null || true
    fi
}
trap cleanup EXIT

"$ROOT/run/ww-loadboot.sh" &
qemu_pid=$!
echo "[lp] qemu pid=$qemu_pid tag=$TAG port=$PORT budget=${SECSN}s"

sleep 20
if ! kill -0 "$qemu_pid" 2>/dev/null; then
    echo "[lp] qemu exited early" >&2
    exit 1
fi
python3 "$ROOT/run/ww-gdbscript.py" "$PORT" > "$GDBCMD"
gdb-multiarch -batch -x "$GDBCMD" > "$GDBLOG" 2>&1 &
gdb_pid=$!
echo "[lp] gdb pid=$gdb_pid armed (19-point hunt set)"

end=$((SECONDS + SECSN))
while [ $SECONDS -lt $end ]; do
    kill -0 "$qemu_pid" 2>/dev/null || break
    sleep 10
done
kill "$qemu_pid" 2>/dev/null || true
wait "$qemu_pid" 2>/dev/null || true
sleep 2
kill "$gdb_pid" 2>/dev/null || true
wait "$gdb_pid" 2>/dev/null || true
gdb_pid=""

echo "[lp] === verdict ($TAG) ==="
echo "[lp] gdb hits:"
grep "WW-HIT" "$GDBLOG" || echo "(none)"
grep -m1 "WW-ARMED" "$GDBLOG" || echo "(MISSING ARM — check gdb log)"
echo "[lp] wildwatch arm:"
grep -m2 "wildwatch" "$SERIAL" || echo "(MISSING — tripwire did not arm)"
echo "[lp] capture markers (serial):"
grep -cE "WILDWATCH-CANARY" "$SERIAL" || true
grep -E "hpet: clock (base|div|offset) healed" "$SERIAL" | head -3 || true
grep -cE "KERNPANIC" "$SERIAL" || true
echo "[lp] load markers:"
grep -m1 "init.wwload.cfg" "$SERIAL" || echo "(import line MISSING — cfg not loaded)"
grep -m1 "starting:wwload" "$SERIAL" || echo "(service start MISSING)"
echo "[lp] WWLOAD-BEAT count: $(grep -c 'WWLOAD-BEAT' "$SERIAL" || true)"
echo "[lp] sdc exec count: $(grep -c 'path=/system/bin/sdc' "$SERIAL" || true)"
echo "[lp] storm health:"
grep -c "fork: parent" "$SERIAL" || true
grep -m1 -oE "WW: wakes=[0-9]+" "$SERIAL" | tail -1 || true
tail -2 "$SERIAL" | cut -c1-140
echo "[lp] serial lines: $(wc -l < "$SERIAL")"
