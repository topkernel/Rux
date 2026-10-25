#!/bin/bash
# ww-loadchain.sh — s31 load-diversification hunt chain (the experiment
# slot next to the main ww-chain2.sh). Boots the PRIVATE sdc-cycle image
# (run/ww31-system.img via ww-loadboot.sh) with the FULL capture stack:
# frozen kernel + dfx=wildwatch + the 19-point gdb watchset.
#
# Load profile: /system/bin/wwload.sh cycles `timeout 2 /system/bin/sdc`
# forever from the post-fs-data job (b77-era trigger context — the
# StorageDaemon fork/exec boundary, ~2-3s per cycle, hundreds of exec
# lifetimes per round) on top of the natural OH storm.
#
# Stop check: same two-log capture markers as ww-chain2.sh (WW-HIT gdb;
# WILDWATCH-CANARY / scribble-heal / KERNPANIC serial) + stop file.
#   ww-loadchain.sh <chain-tag> [runs] [gdbport] [first]
#   touch run/<chain-tag>.stop
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TAG="${1:-wwload}"
RUNS="${2:-100}"
PORT="${3:-12381}"
i="${4:-1}"
LAST=$((i + RUNS - 1))
STOP="$ROOT/run/$TAG.stop"
rm -f "$STOP"

while [ "$i" -le "$LAST" ]; do
    [ -f "$STOP" ] && { echo "[lchain] stop file, ending after $((i-1)) runs"; break; }
    echo "[lchain] $(date +%H:%M:%S) run $i/$RUNS starting (port $PORT)" >&2
    "$ROOT/run/ww-loadprobe.sh" "${TAG}-r${i}" 2700 "$PORT" >&2 || {
        echo "[lchain] run $i failed rc=$? — continuing" >&2
    }
    G="$ROOT/run/${TAG}-r${i}-gdb.log"
    S="$ROOT/run/${TAG}-r${i}-serial.log"
    HIT=""
    # HEAL_LAST adjudication (s4, 2026-10-25 — see ww-chain2.sh and
    # run/fleet/evidence/2026-10-25-heallast-hit-evidence.txt):
    # HEAL_LAST-ONLY gdb hit + "clock corrected backward" in the serial
    # and no scribble line => benign jitter-clamp caller, keep hunting.
    HEAL_ONLY="no"
    if [ -f "$G" ] && grep -q "WW-HIT" "$G"; then
        OTHER=$(grep "WW-HIT" "$G" | grep -cv "WW-HIT HEAL_LAST" || true)
        if [ "$OTHER" = "0" ] && grep -q "WW-HIT HEAL_LAST" "$G"; then
            HEAL_ONLY="yes"
        fi
        HIT="WW-HIT"
        echo "[lchain] WW-HIT in run $i — CAPTURE. gdb log: $G" >&2
        grep "WW-HIT" "$G" | head -5 >&2
    fi
    if [ -f "$S" ]; then
        if grep -q "WILDWATCH-CANARY" "$S"; then
            HIT="${HIT:+$HIT+}WILDWATCH-CANARY"
            echo "[lchain] WILDWATCH-CANARY in run $i — CAPTURE. serial: $S" >&2
            grep "WILDWATCH-CANARY" "$S" | head -3 >&2
        fi
        if grep -qE "hpet: clock (base|div|offset) healed" "$S"; then
            HIT="${HIT:+$HIT+}SCRIBBLE-HEAL"
            echo "[lchain] scribble-heal in run $i — CAPTURE. serial: $S" >&2
            grep -E "hpet: clock (base|div|offset) healed" "$S" | head -3 >&2
        fi
        if grep -q "KERNPANIC" "$S"; then
            HIT="${HIT:+$HIT+}KERNPANIC"
            echo "[lchain] KERNPANIC in run $i — CAPTURE/investigate. serial: $S" >&2
            grep -m3 "KERNPANIC" "$S" >&2
        fi
        grep -m1 "wildwatch armed" "$S" >&2 || \
            echo "[lchain] NOTE: no wildwatch-armed line in run $i (arm failed?)" >&2
        # load sanity: the sdc cycles must be present (missing load = the
        # private image stopped being used, or wwload died).
        n=$(grep -c 'path=/system/bin/sdc' "$S" 2>/dev/null || echo 0)
        echo "[lchain] sdc cycles this round: $n" >&2
        if [ "${n:-0}" -lt 50 ]; then
            echo "[lchain] NOTE: sdc cycle count low ($n) — load not running?" >&2
        fi
    fi
    if [ "$HEAL_ONLY" = "yes" ] && [ -f "$S" ] \
       && grep -q "clock corrected backward" "$S" \
       && ! grep -qE "hpet: clock (base|div|offset) healed" "$S" \
       && [ "$HIT" = "WW-HIT" ]; then
        echo "[lchain] WW-HIT HEAL_LAST adjudicated BENIGN-JITTER (clock corrected backward in serial, no scribble) — continuing" >&2
        HIT=""
    fi
    if [ -n "$HIT" ]; then
        echo "[lchain] CAPTURE markers ($HIT) in run $i — stopping chain." >&2
        touch "$STOP"
    fi
    i=$((i+1))
done
echo "[lchain] done: $((i-1)) runs" >&2
