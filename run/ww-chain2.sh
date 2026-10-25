#!/bin/bash
# ww-chain2.sh — wildwrite hunt chain, session-30 successor of ww-chain.sh
# (that file stays untouched because a live driver may be executing it;
# this one is the same loop with the wildwatch serial capture markers
# added to the per-round stop check).
#
# Per round the hunt boot now carries dfx=wildwatch (set by ww-hunt.py):
# the gdb watchpoints stay the rip-attributing surface for the 19 watched
# statics OUTSIDE the guard cluster, while the cluster pages are covered
# by the kernel RO trap (#PF at the writer's RIP -> KERNPANIC dump) and
# the 1 s canary cadence (PTE-invisible writers). Those captures land in
# the SERIAL log, not the gdb log — so the stop check greps both.
#
# Usage / stop file: identical to ww-chain.sh.
#   ww-chain2.sh <chain-tag> [runs] [gdbport] [first]
#   touch run/<chain-tag>.stop
set -u

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TAG="${1:-ww-s2}"
RUNS="${2:-100}"
PORT="${3:-12371}"
i="${4:-1}"
LAST=$((i + RUNS - 1))
STOP="$ROOT/run/$TAG.stop"
rm -f "$STOP"

while [ "$i" -le "$LAST" ]; do
    [ -f "$STOP" ] && { echo "[chain] stop file, ending after $((i-1)) runs"; break; }
    echo "[chain] $(date +%H:%M:%S) run $i/$RUNS starting (port $PORT)" >&2
    python3 "$ROOT/run/ww-hunt.py" "${TAG}-r${i}" 2700 "$PORT" >&2 || {
        echo "[chain] run $i failed rc=$? — continuing" >&2
    }
    G="$ROOT/run/${TAG}-r${i}-gdb.log"
    S="$ROOT/run/${TAG}-r${i}-serial.log"
    HIT=""
    # gdb watchpoint surface (rip + regs + stack in the gdb log).
    # HEAL_LAST adjudication (s4, 2026-10-25, after r96/load-r43):
    # HEAL_LAST is heal_report's rate-gate static, shared by ALL FOUR
    # callers — three scribble classes AND the benign "clock corrected
    # backward" jitter/correction clamp. A HEAL_LAST-ONLY hit whose
    # serial carries "corrected backward" (and no scribble line) is the
    # benign caller: log it and CONTINUE. Anything else still stops.
    # (HEAL_LAST-only with NO heal line at all still stops — manual
    # adjudication; the print can be lost to serial buffering.)
    HEAL_ONLY="no"
    if [ -f "$G" ] && grep -q "WW-HIT" "$G"; then
        OTHER=$(grep "WW-HIT" "$G" | grep -cv "WW-HIT HEAL_LAST" || true)
        if [ "$OTHER" = "0" ] && grep -q "WW-HIT HEAL_LAST" "$G"; then
            HEAL_ONLY="yes"
        fi
        HIT="WW-HIT"
        echo "[chain] WW-HIT in run $i — CAPTURE. gdb log: $G" >&2
        grep "WW-HIT" "$G" | head -5 >&2
    fi
    # wildwatch surfaces (serial): canary corruption (DMA-class), the
    # scribbled-pair heals (PTE-invisible writer hit the MEMBERS page),
    # and any kernel panic — the RO trap's KERNPANIC dump carries rip +
    # rbp walk + stack; symbolize with addr2line on the frozen ELF.
    # ("clock corrected backward" is the jitter clamp, NOT a capture.)
    if [ -f "$S" ]; then
        if grep -q "WILDWATCH-CANARY" "$S"; then
            HIT="${HIT:+$HIT+}WILDWATCH-CANARY"
            echo "[chain] WILDWATCH-CANARY in run $i — CAPTURE. serial: $S" >&2
            grep "WILDWATCH-CANARY" "$S" | head -3 >&2
        fi
        if grep -qE "hpet: clock (base|div|offset) healed" "$S"; then
            HIT="${HIT:+$HIT+}SCRIBBLE-HEAL"
            echo "[chain] scribble-heal in run $i — CAPTURE (PTE-invisible writer?). serial: $S" >&2
            grep -E "hpet: clock (base|div|offset) healed" "$S" | head -3 >&2
        fi
        if grep -q "KERNPANIC" "$S"; then
            HIT="${HIT:+$HIT+}KERNPANIC"
            echo "[chain] KERNPANIC in run $i — CAPTURE/investigate. serial: $S" >&2
            grep -m3 "KERNPANIC" "$S" >&2
        fi
        # Anomaly note (non-fatal): the tripwire must be armed every boot.
        grep -m1 "wildwatch armed" "$S" >&2 || \
            echo "[chain] NOTE: no wildwatch-armed line in run $i (arm failed?)" >&2
    fi
    # Benign HEAL_LAST adjudication: only-HEAL_LAST gdb hit + "corrected
    # backward" in the serial + NO scribble/canary/panic marker => the
    # jitter-clamp caller; demote the hit and keep hunting.
    if [ "$HEAL_ONLY" = "yes" ] && [ -f "$S" ] \
       && grep -q "clock corrected backward" "$S" \
       && ! grep -qE "hpet: clock (base|div|offset) healed" "$S" \
       && [ "$HIT" = "WW-HIT" ]; then
        echo "[chain] WW-HIT HEAL_LAST adjudicated BENIGN-JITTER (clock corrected backward in serial, no scribble) — continuing" >&2
        HIT=""
    fi
    if [ -n "$HIT" ]; then
        echo "[chain] CAPTURE markers ($HIT) in run $i — stopping chain." >&2
        touch "$STOP"
    fi
    i=$((i+1))
done
echo "[chain] done: $((i-1)) runs" >&2
