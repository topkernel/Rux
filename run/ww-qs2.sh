#!/bin/bash
# ww-qs2.sh — storm-quietness probe for the s30 watchpoint-repurpose
# candidates (run/ww-qs2-gdbscript.py's 8), full OH storm boot with
# dfx=wildwatch ON (chain coexistence validation: the tripwire must arm
# and the boot must survive the storm with 8 watchpoints + the RO cluster).
# qs1 shape: ~15.5 min budget, own port, own PIDs, -snapshot +
# file.locking=off via ww-boot.sh. A candidate that HITS is not
# chain-safe; zero hits across the storm = safe to arm on the chain.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TAG=ww-qs2
PORT=12377
SECSN=930

SERIAL="$ROOT/run/${TAG}-serial.log"
GDBLOG="$ROOT/run/${TAG}-gdb.log"
GDBCMD="$ROOT/run/${TAG}-gdb.cmd"

export TAG GDBPORT=$PORT SECS=$SECSN
export EXTRA_APPEND="dfx=wildwatch"

# Own QEMU, budget-enforced by timeout inside ww-boot.sh.
qemu_pid=""
gdb_pid=""
cleanup() {
    # Kill ONLY the PIDs this script started.
    if [ -n "$gdb_pid" ] && kill -0 "$gdb_pid" 2>/dev/null; then
        kill "$gdb_pid" 2>/dev/null || true
    fi
    if [ -n "$qemu_pid" ] && kill -0 "$qemu_pid" 2>/dev/null; then
        kill "$qemu_pid" 2>/dev/null || true
    fi
}
trap cleanup EXIT

"$ROOT/run/ww-boot.sh" &
qemu_pid=$!
echo "[qs2] qemu pid=$qemu_pid port=$PORT tag=$TAG budget=${SECSN}s"

sleep 20
if ! kill -0 "$qemu_pid" 2>/dev/null; then
    echo "[qs2] qemu exited early" >&2
    exit 1
fi
python3 "$ROOT/run/ww-qs2-gdbscript.py" "$PORT" > "$GDBCMD"
gdb-multiarch -batch -x "$GDBCMD" > "$GDBLOG" 2>&1 &
gdb_pid=$!
echo "[qs2] gdb pid=$gdb_pid armed (8 candidates)"

# Supervise to budget; ww-boot.sh's timeout ends QEMU on its own.
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
gdb_pid=""; qemu_pid=""

echo "[qs2] done. hits per candidate:"
grep "WW-HIT" "$GDBLOG" || echo "(none)"
echo "[qs2] arm status:"
grep "WW-ARMED" "$GDBLOG" || echo "(MISSING — check gdb log)"
echo "[qs2] wildwatch arm:"
grep -m2 "wildwatch" "$SERIAL" || echo "(MISSING — tripwire did not arm)"
echo "[qs2] panics:"
grep -c "KERNPANIC" "$SERIAL" || true
echo "[qs2] serial lines: $(wc -l < "$SERIAL")"
