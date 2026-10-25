#!/bin/bash
# fleet-classify-round.sh — classify one finished fleet round.
# Usage: fleet-classify-round.sh <member> <round> <serial> <rc> <wedge>
# Prints a one-line summary (also appended by the member driver).
# Class order: CRITICAL > WARN > info. Pattern families mirror the
# hunt ledger + standing-lane mission (panics, wild writes, watchdog,
# OOM, scheduler pathologies, memory growth, data corruption).
set -u
MEMBER="$1"; ROUND="$2"; SERIAL="$3"; RC="$4"; WEDGE="${5:-no}"

[ -f "$SERIAL" ] || { echo "R$ROUND no-serial rc=$RC"; exit 0; }
LINES=$(wc -l < "$SERIAL")

hits() { grep -c "$1" "$SERIAL" 2>/dev/null || true; }

CRIT=""
WARN=""

# --- CRITICAL: kernel-side failure signatures -------------------------
# (session-3 tightening: bare "page fault" matched thread-exit diagnostic
# dumps' fault counters — e.g. ltp-a r22's writev03 dump — producing a
# phantom OOPS=1 on clean v5/v7 rounds. Require kernel-mode phrasing.)
kernpanic=$(grep -c "KERNPANIC" "$SERIAL" 2>/dev/null)
oosp=$(grep -ciE "kernel oops|BUG: (soft|hard)|Unable to handle|page fault in (kernel|supervisor)|PANIC: trap" "$SERIAL" 2>/dev/null)
canary=$(grep -c "WILDWATCH-CANARY" "$SERIAL" 2>/dev/null)
heal=$(grep -cE "hpet: clock (base|div|offset) healed" "$SERIAL" 2>/dev/null)
wdog=$(grep -ciE "watchdog.*trip|deadlock detected|spinlock.*stuck" "$SERIAL" 2>/dev/null)
[ "$kernpanic" -gt 0 ] 2>/dev/null && CRIT="${CRIT}KERNPANIC=$kernpanic "
[ "$oosp" -gt 0 ] 2>/dev/null && CRIT="${CRIT}OOPS=$oosp "
[ "$canary" -gt 0 ] 2>/dev/null && CRIT="${CRIT}CANARY=$canary "
[ "$heal" -gt 0 ] 2>/dev/null && CRIT="${CRIT}SCRIBBLE-HEAL=$heal "
[ "$wdog" -gt 0 ] 2>/dev/null && CRIT="${CRIT}WATCHDOG=$wdog "

# --- WARN: anomaly candidates ------------------------------------------
# OOM lines, excluding the image-side known class (init userspace write
# failures to oom_score_adj paths on OH boots).
oom=$(grep -iE "out of memory|oom-kill|invoked oom" "$SERIAL" 2>/dev/null | grep -vc "oom_score_adj" || true)
[ "$oom" -gt 0 ] 2>/dev/null && WARN="${WARN}OOM=$oom "

# IRQ-off windows beyond the canonical family (x86 chains): any sample
# with age > 2000ms is a new-class candidate.
irq=$(grep -oE "IRQ-OFF-WINDOW.*age ([0-9]+)" "$SERIAL" 2>/dev/null | grep -oE "[0-9]+$" | sort -n | tail -1)
[ -n "$irq" ] && [ "$irq" -gt 2000 ] 2>/dev/null && WARN="${WARN}IRQ-OFF-MAX=${irq}ms "

# scheduler census pathologies (if census probes armed)
census=$(grep -cE "WAKE-STRANDED [1-9][0-9]{2,}|TERM-DESCHED [0-9]{3,}" "$SERIAL" 2>/dev/null)
[ "$census" -gt 0 ] 2>/dev/null && WARN="${WARN}CENSUS-ANOM=$census "

# probe self-reported anomalies
odd=$(grep -c "FSFUZZ-ODD" "$SERIAL" 2>/dev/null)
[ "$odd" -gt 20 ] 2>/dev/null && WARN="${WARN}FSFUZZ-ODD=$odd "
ffail=$(grep -c "MEMSTORM-FORKFAIL" "$SERIAL" 2>/dev/null)
[ "$ffail" -gt 10 ] 2>/dev/null && WARN="${WARN}FORKFAIL=$ffail "
enomap=$(grep -c "MEMSTORM-ENOMAP" "$SERIAL" 2>/dev/null)
[ "$enomap" -gt 20 ] 2>/dev/null && WARN="${WARN}ENOMAP=$enomap "

# memory growth: MEM heap <usedK>K/<sizeK> lines (dfx=periodic,memwatch)
heap_first=$(grep -oE "^MEM heap [0-9]+" "$SERIAL" 2>/dev/null | head -1 | grep -oE "[0-9]+")
heap_last=$(grep -oE "^MEM heap [0-9]+" "$SERIAL" 2>/dev/null | tail -1 | grep -oE "[0-9]+")
if [ -n "$heap_first" ] && [ -n "$heap_last" ]; then
    growth=$(( heap_last - heap_first ))
    [ "$growth" -gt 65536 ] && WARN="${WARN}HEAP-GROW=${growth}KiB "
fi

# LTP sweep result mix (riscv LTP members)
tres=$(grep -c "SWEEP RES" "$SERIAL" 2>/dev/null)
twedge=$(grep -cE "SWEEP RES\|[^|]*\|[^|]*\|(WEDGE|HANG)" "$SERIAL" 2>/dev/null)

# monkey/ltp progress heartbeat
beat=$(grep -cE "MONKEY-BEAT|MEMSTORM-BEAT|FSFUZZ-BEAT" "$SERIAL" 2>/dev/null)

# known image-side noise tally (context only, never WARN)
noise_usb=$(grep -ciE "usb_host.*(crash|died)|CrashDumpEntry.*usb_host" "$SERIAL" 2>/dev/null)
noise_samgr=$(grep -cE "SAMgr.*(15|30|89)s|samgr.*cost" "$SERIAL" 2>/dev/null)

# PREEMPT-LEAK: since the 2026-10-25 print gate, any line that survives
# carries non-self bits (0x20000+ or PREEMPT/SOFTIRQ/NMI base) — a real
# held-count candidate. Count in the summary line; WARN at volume.
pleak=$(grep -c "PREEMPT-LEAK" "$SERIAL" 2>/dev/null)
[ "$pleak" -gt 5 ] 2>/dev/null && WARN="${WARN}PREEMPT-LEAK=$pleak "

# PARKED-LEAK: tracked open class (fires on ~47% of OH boots across
# lanes; samgr-callback serials show it since sc-era). Context counter,
# logged per round for the ledger; promoted to WARN only if it appears
# on NON-OH members (riscv64 fleet members) where it has never been seen.
park=$(grep -c "PARKED-LEAK" "$SERIAL" 2>/dev/null)
if [ "$park" -gt 0 ] 2>/dev/null && ! grep -q "ohos.boot" "$SERIAL" 2>/dev/null; then
    WARN="${WARN}PARKED-LEAK-RV=$park "
fi

if [ -n "$CRIT" ]; then
    CLS="CRITICAL"
elif [ -n "$WARN" ]; then
    CLS="WARN"
elif [ "$WEDGE" = "yes" ]; then
    CLS="WEDGE"
else
    CLS="clean"
fi

echo "R$ROUND $CLS lines=$LINES rc=$RC wedge=$WEDGE beats=$beat sweepRES=$tres sweepWEDGE=$twedge ${CRIT}${WARN}noise:usb=$noise_usb,samgr=$noise_samgr,park=$park heap:$heap_first->$heap_last KiB"
[ -n "$CRIT" ] || [ -n "$WARN" ] && echo "  serial: $SERIAL (preserve for sweep)"
exit 0
