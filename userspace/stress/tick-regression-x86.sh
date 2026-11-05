#!/bin/bash
# tick-regression-x86.sh — SP-5 round-5 regression test: the timer IRQs
# must keep flowing through the memory-exhaustion cascade.
#
# Reproducer class: malloc 4x256M on the 2G image (the round-4/5
# deterministic SP-5 face). Before the fix (8884c2b3): the OOM-FORENSIC
# clock lines showed tb advancing while per-CPU tick-IRQ counters froze
# (~48/52) for the whole cascade — jiffies, OOM kill windows, watchdog
# and every jiffies-derived census died with them. After: the counters
# advance across every dump.
#
# Verdict rules (serial log of ONE boot):
#   PASS  every OOM-FORENSIC clock line's irq0/irq1 pair is >= the
#         previous line's (ticks never go backward), AND the LAST line
#         is strictly greater than the FIRST on at least one CPU (ticks
#         flowed through the cascade), AND no TICKTRAP line pairs
#         verdict=irr-stuck/lvt-masked with a frozen tickirq counter.
#   FAIL  otherwise (frozen-counter-while-tb-advances signature).
#   SKIP  no OOM-FORENSIC dumps in the boot (the cascade did not fire —
#         rerun; the face is deterministic on this class).
set -u
cd "$(dirname "$0")/../.."

LOG="${1:-run/stress-runs/tick-regression.log}"
BUDGET="${BUDGET:-300}"
debug="${debug:-debug=leakhunt,watchdog}"

echo "tick-regression: malloc 4x256M boot (budget ${BUDGET}s)"
if [ "${VERDICT_ONLY:-0}" != 1 ] || [ ! -f "$LOG" ]; then
    BUDGET="$BUDGET" debug="$debug" OUTDIR="$(dirname "$LOG")" \
        test/stressng/run-class-x86.sh --malloc 4 --timeout 100 --vm-bytes 256M \
        > /dev/null 2>&1
    # run-class writes run/stress-runs/malloc.log; the caller may have set
    # OUTDIR — normalize both.
    SRC="$(dirname "$LOG")/malloc.log"
    [ -f "$SRC" ] || SRC=run/stress-runs/malloc.log
    cp "$SRC" "$LOG" 2>/dev/null || true
fi

# --- rule 1: clock-line monotonicity + net advance ---------------------
# NOTE: extract fields by name — the bare "[0-9]+" split of the earlier
# revision also matched the digits inside the field NAMES irq0/irq1 and
# mis-grouped the columns (r6 lesson, tickreg-fix2 boot).
mapfile -t CLK < <(grep -aoE "clock tb=[0-9]+ irq0=[0-9]+ irq1=[0-9]+" "$LOG" \
    | sed -E 's/.*tb=([0-9]+) irq0=([0-9]+) irq1=([0-9]+)/\1 \2 \3/')

if [ "${#CLK[@]}" -eq 0 ]; then
    echo "tick-regression: SKIP (no OOM-FORENSIC dumps; cascade did not fire)"
    exit 2
fi
echo "tick-regression: ${#CLK[@]} clock lines"

prev0=-1; prev1=-1; mono=OK
first0=-1; first1=-1
for line in "${CLK[@]}"; do
    read -r tb i0 i1 <<<"$line"
    if [ "$first0" -lt 0 ]; then first0=$i0; first1=$i1; fi
    if [ "$i0" -lt "$prev0" ] || [ "$i1" -lt "$prev1" ]; then mono=VIOLATED; fi
    prev0=$i0; prev1=$i1
done
last0=$prev0; last1=$prev1
echo "tick-regression: first irq=($first0,$first1) last irq=($last0,$last1) monotonic=$mono"

if [ "$mono" != OK ]; then
    echo "tick-regression: FAIL (tick counters went backward)"
    exit 1
fi
if [ "$last0" -le "$first0" ] && [ "$last1" -le "$first1" ]; then
    echo "tick-regression: FAIL (tick counters frozen across the cascade — the round-4 dead-tick signature)"
    exit 1
fi

# --- rule 2: no irr-stuck/lvt-masked trap verdict with frozen counters --
BAD=$(grep -acE "TICKTRAP.*(verdict=(irr-stuck|lvt-masked|apic-disabled|tpr-blocked))" "$LOG" || true)
if [ "$BAD" -gt 0 ]; then
    echo "tick-regression: FAIL ($BAD TICKTRAP delivery-blocked verdicts)"
    grep -aE "TICKTRAP.*(verdict=(irr-stuck|lvt-masked))" "$LOG" | head -3
    exit 1
fi

echo "tick-regression: PASS (timer IRQs alive through the exhaustion cascade; log: $LOG)"
exit 0
